// SPDX-License-Identifier: AGPL-3.0-only

//! The cohorts' list as the door answers it (2026-11-12), for the desk's
//! Data page. Every request used to gather every cohort's counts, its
//! intervals, its holdings, its releases and every open review item: 2 to 6
//! seconds each time on a registry of some thousands of open items, and
//! the page asks on every visit.
//!
//! The list is kept ([`crate::kept`], as a dataset's summary keeps its
//! counts): answered at once whatever its age and gathered again apart,
//! one gathering at a time, where its stamp moved or its keep ran out,
//! and kept in `registry_meta` (`cohorts_list`) for the next start of the
//! engine. Each cohort says whether it is counted again (`counting`) and
//! when it was counted (`counts_as_of`). Only a registry whose list was
//! never gathered makes a request wait, once.
//!
//! What is cheap is read on every request and laid over the kept list: a
//! cohort's own row (its name, owner, description, whether it is retired)
//! and the datasets feeding it. A cohort the kept list does not hold yet is
//! read by itself, now. So a cohort made, renamed or retired shows in the
//! very next answer, and so does one given or losing members at its door,
//! which reads that cohort again before it answers ([`member_change`]).
//!
//! What moves the stamp ([`stamp`]), so starts a gathering apart at the
//! next request: the cohorts' rows, the active places' rows, a member
//! joined or left, a review item raised or closed, a release made,
//! finished or withdrawn, a handover, a batch begun or ended, the newest
//! finished job that is no worker, a merge of subjects, a sample sealed or
//! unsealed, the session cache rebuilt, and the number of the counting
//! ([`COUNTING`]), never the engine's version. Whatever else changes a
//! count shows within the keep's ten minutes, one minute while a batch
//! runs, after which the next request starts a gathering apart.
//!
//! The list is the same for every caller: an item wholly about a sample
//! sealed now waits for nobody, whoever asks, so one keep answers all.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

use nils_registry::cohort::{self, Cohort};
use nils_registry::place::{self, Place};
use nils_registry::store::{Error as StoreError, Store};
use nils_registry::{Registry, job};
use serde_json::{Value, json};

use crate::kept::{Ask, Keep, Restored};

/// The number of the counting: what the list's kept documents are and how
/// they are gathered (`nils_registry::cohort::list`). Bump it by hand in
/// the change that makes the list say something else or count it
/// differently, so that a list an engine before kept is gathered again
/// once, apart, while it is still answered.
const COUNTING: u32 = 1;

/// How long the list is kept while its stamp is the same: the bound on
/// what moves a count without moving the stamp.
const KEEP: Duration = Duration::from_secs(600);

/// And while a batch runs, whose stacks arrive as it goes.
const KEEP_BUSY: Duration = Duration::from_secs(60);

/// The `registry_meta` key the list is kept under between starts of the
/// engine.
const STORED: &str = "cohorts_list";

static KEPT: LazyLock<Keep<Value>> = LazyLock::new(Default::default);

/// A registry's slot in the keep: an engine serves one registry, and the
/// tests many.
fn slot_of(registry: &Registry) -> String {
    format!("list:{}", registry.home().dir().display())
}

/// What the list is read under, from indexes and small tables, and
/// whether a batch runs now.
fn stamp(
    store: &mut Store,
    cohorts: &[Cohort],
    places: &[Place],
) -> Result<(String, bool), StoreError> {
    let q = |t: &str| store.qualified(t);
    let (member, item, release, handover) = (
        q("cohort_member"),
        q("review_item"),
        q("release"),
        q("handover"),
    );
    let (batch, subject, sealed, session, jobs) = (
        q("ingest_batch"),
        q("subject"),
        q("sealed_stack"),
        q("session_cache"),
        q("job"),
    );
    let workers = format!(
        "'worker', '{}', '{}'",
        job::PIPELINE_WORKER,
        job::PICTURE_WORKER
    );
    let sql = format!(
        "SELECT (SELECT COUNT(*) FROM {member}), (SELECT MAX(id) FROM {member}), \
         (SELECT COUNT(*) FROM {member} WHERE left_at IS NULL), \
         (SELECT MAX(id) FROM {item}), (SELECT COUNT(*) FROM {item} WHERE status = 'open'), \
         (SELECT COUNT(*) FROM {release}), (SELECT MAX(id) FROM {release}), \
         (SELECT COUNT(*) FROM {release} WHERE finished_at IS NOT NULL), \
         (SELECT COUNT(*) FROM {release} WHERE withdrawn_at IS NOT NULL), \
         (SELECT COUNT(*) FROM {handover}), \
         (SELECT COUNT(*) FROM {batch}), (SELECT MAX(id) FROM {batch}), \
         (SELECT COUNT(*) FROM {batch} WHERE finished_at IS NULL), \
         (SELECT MAX(id) FROM {jobs} WHERE finished_at IS NOT NULL AND kind NOT IN ({workers})), \
         (SELECT COUNT(*) FROM {subject} WHERE merged_into IS NOT NULL), \
         (SELECT COUNT(*) FROM {sealed} WHERE unsealed_at IS NULL), \
         (SELECT MAX(id) FROM {session}), (SELECT COUNT(*) FROM {session})"
    );
    let r = &store.query(&sql, &[])?[0];
    let read = (0..18)
        .map(|i| Ok(r.opt_int(i)?.unwrap_or(0).to_string()))
        .collect::<Result<Vec<_>, StoreError>>()?;
    let running = r.opt_int(12)?.unwrap_or(0);
    let rows: Vec<Value> = cohorts
        .iter()
        .map(|c| json!([c.id, c.name, c.owner, c.description, c.retired_at]))
        .collect();
    let places: Vec<Value> = places.iter().map(Place::as_json).collect();
    Ok((
        format!(
            "counting {COUNTING}|{}|{}|{}",
            read.join("|"),
            Value::Array(rows),
            Value::Array(places)
        ),
        running > 0,
    ))
}

/// One gathering of the list.
struct Gather;

impl crate::kept::Reading<Value> for Gather {
    type Error = StoreError;

    fn failure(error: &StoreError) -> String {
        error.to_string()
    }

    fn read(&mut self, registry: &mut Registry) -> Result<Value, StoreError> {
        Ok(Value::Array(cohort::list(registry.store())?))
    }

    fn stored(&self, stamp: &str, as_of: &str, value: &Value) -> Option<String> {
        Some(json!({"stamp": stamp, "as_of": as_of, "list": value}).to_string())
    }
}

/// The list the registry keeps, read once after a start of the engine.
fn stored_read(text: &str) -> Option<Restored<Value>> {
    let stored: Value = serde_json::from_str(text).ok()?;
    if !stored["list"].is_array() {
        return None;
    }
    Some(Restored {
        stamp: stored["stamp"].as_str()?.to_string(),
        as_of: stored["as_of"].as_str()?.to_string(),
        value: stored["list"].clone(),
    })
}

/// Every cohort with its counts and provenance, as `GET /api/cohorts`
/// answers: the kept list at once, each cohort's own row and the datasets
/// feeding it as they are now, and whether it is counted again.
pub(crate) fn list(registry: &mut Registry) -> Result<Value, StoreError> {
    let slot = slot_of(registry);
    let store = registry.store();
    let cohorts = cohort::all(store)?;
    let places = place::active(store)?;
    let (stamp, busy) = stamp(store, &cohorts, &places)?;
    let answer = KEPT.answer(
        registry,
        &Ask {
            slot: &slot,
            stored: STORED,
            stamp: &stamp,
            ttl: if busy { KEEP_BUSY } else { KEEP },
            paced: false,
            wait: true,
            fresh: false,
        },
        stored_read,
        || Gather,
    )?;
    let none = Vec::new();
    let kept: HashMap<i64, &Value> = answer
        .value
        .as_ref()
        .and_then(|v| v.as_array())
        .unwrap_or(&none)
        .iter()
        .filter_map(|doc| Some((doc["id"].as_i64()?, doc)))
        .collect();
    let mut out = Vec::with_capacity(cohorts.len());
    for c in &cohorts {
        let doc = match kept.get(&c.id) {
            Some(doc) => {
                let mut doc = (*doc).clone();
                cohort::laid_now(&mut doc, &places, c);
                doc["counting"] = json!(answer.counting);
                doc["counts_as_of"] = json!(answer.as_of);
                doc
            }
            // made since the list was gathered: read by itself, now
            None => {
                let mut doc = cohort::listed(registry.store(), c)?;
                doc["counting"] = json!(false);
                doc["counts_as_of"] = json!(nils_registry::time::now_iso());
                doc
            }
        };
        out.push(doc);
    }
    Ok(Value::Array(out))
}

/// Mark the kept list stale: it is gathered again at the next request,
/// which it still answers.
pub(crate) fn forget() {
    KEPT.forget();
}

/// A door gave a cohort members or took some away: the cohort is read
/// again by itself and put in the kept list before the door answers, so
/// the very next list says its new counts, and the whole list is gathered
/// again apart at that request. A gathering under way, which began before
/// the door's write, is not kept.
///
/// It gathers nothing itself (2026-11-12): the registry's copy is taken
/// over where this engine has not looked for it yet, and the next request
/// gathers the whole list once, apart. Where nothing is kept there is
/// nothing to put the cohort in, and the next request gathers in front.
pub(crate) fn member_change(registry: &mut Registry, name: &str) -> Result<(), StoreError> {
    KEPT.taken_over(registry, &slot_of(registry), STORED, KEEP, stored_read);
    let Some(c) = cohort::by_name(registry.store(), name)? else {
        return Ok(());
    };
    let fresh = cohort::listed(registry.store(), &c)?;
    KEPT.amend(&slot_of(registry), |list| {
        let mut all = list.as_array().cloned().unwrap_or_default();
        match all.iter_mut().find(|doc| doc["id"].as_i64() == Some(c.id)) {
            Some(doc) => *doc = fresh,
            None => all.push(fresh),
        }
        Value::Array(all)
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nils_registry::home::{Home, InitOptions};

    struct MadeUp {
        _dir: nils_dicom::synth::TempDir,
        home: Home,
        registry: Registry,
    }

    /// A registry with three subjects and one cohort, `first`, nobody in
    /// it yet.
    fn made_up(name: &str) -> MadeUp {
        let dir = nils_dicom::synth::TempDir::new(name);
        let home = Home::new(dir.path().join("registry"));
        home.keys(None).add("k", b"nils-fixture-key").unwrap();
        let mut registry = home
            .init(&InitOptions {
                backend: nils_registry::Backend::Sqlite,
                dsn: None,
                schema: None,
                scheme: nils_registry::pseudonym::Scheme::Blake2b32,
                key: "k".to_string(),
                display_length: nils_registry::pseudonym::DEFAULT_DISPLAY_LENGTH,
                session_scheme: None,
            })
            .unwrap();
        registry
            .store()
            .execute(
                "INSERT INTO subject (id, code, created_at) \
                 VALUES (1, 'sub-one', 't'), (2, 'sub-two', 't'), (3, 'sub-three', 't')",
                &[],
            )
            .unwrap();
        cohort::create(&mut registry, "first", "anna@lab", None, "anna@lab").unwrap();
        MadeUp {
            _dir: dir,
            home,
            registry,
        }
    }

    impl MadeUp {
        fn list(&mut self) -> Value {
            super::list(&mut self.registry).unwrap()
        }

        /// How many times the list was gathered.
        fn reads(&self) -> usize {
            KEPT.reads_of(&slot_of(&self.registry))
        }

        fn settled(&self) {
            KEPT.settled(&slot_of(&self.registry));
        }

        /// Members joined as a digest's feed or another process joins
        /// them: no door of this engine heard of it.
        fn joined(&mut self, cohort: &str, codes: &[&str]) {
            let add: Vec<String> = codes.iter().map(|c| c.to_string()).collect();
            cohort::members(&mut self.registry, cohort, &add, &[], None, "anna@lab").unwrap();
        }
    }

    fn of<'a>(list: &'a Value, name: &str) -> &'a Value {
        list.as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("no {name} in {list}"))
    }

    /// 2026-11-12: the list is gathered once and answered from then on
    /// whatever moves: at once as kept, said to be counted again, while one
    /// gathering runs apart, and with the new counts once it is over.
    #[test]
    fn the_list_is_kept_and_gathered_again_apart_where_its_stamp_moved() {
        let mut m = made_up("cohorts-kept");
        let first = m.list();
        assert_eq!(m.reads(), 1);
        assert_eq!(first.as_array().unwrap().len(), 1);
        let c = of(&first, "first");
        assert_eq!(c["subjects"], 0, "{c}");
        assert_eq!(c["counting"], false, "{c}");
        assert!(c["counts_as_of"].as_str().unwrap().starts_with("20"));
        // what is kept is what a gathering of the whole list says
        let mut whole = cohort::list(m.registry.store()).unwrap();
        for (doc, kept) in whole.iter_mut().zip(first.as_array().unwrap()) {
            doc["counting"] = kept["counting"].clone();
            doc["counts_as_of"] = kept["counts_as_of"].clone();
        }
        assert_eq!(Value::Array(whole), first);
        // kept: a second request gathers nothing, and neither does one
        // after the registry's epoch alone moved
        m.registry.next_epoch().unwrap();
        assert_eq!(m.list(), first);
        assert_eq!(m.reads(), 1, "kept");
        // members joined behind the doors move the stamp: the kept counts
        // at once, said to be counted again, and one gathering apart for
        // however many requests arrive
        m.joined("first", &["sub-one", "sub-two"]);
        let stale = m.list();
        let c = of(&stale, "first");
        assert_eq!(c["subjects"], 0, "{c}");
        assert_eq!(c["counting"], true, "{c}");
        assert_eq!(c["counts_as_of"], of(&first, "first")["counts_as_of"]);
        m.list();
        m.list();
        m.settled();
        assert_eq!(m.reads(), 2, "one gathering for three requests");
        let fresh = m.list();
        let c = of(&fresh, "first");
        assert_eq!(c["subjects"], 2, "{c}");
        assert_eq!(c["counting"], false, "{c}");
        assert_eq!(m.reads(), 2);
        // the keep running out starts one gathering apart as well
        KEPT.aged(&slot_of(&m.registry));
        let aged = m.list();
        assert_eq!(of(&aged, "first")["subjects"], 2);
        assert_eq!(of(&aged, "first")["counting"], true);
        m.settled();
        assert_eq!(m.reads(), 3);
        assert_eq!(of(&m.list(), "first")["counting"], false);
        assert_eq!(m.reads(), 3);
    }

    /// 2026-11-12: what a door writes shows in the very next answer. A
    /// cohort made is read by itself, a cohort renamed, described or
    /// retired is its own row read now, and a cohort given members at its
    /// door was read again before the door answered.
    #[test]
    fn a_cohort_made_renamed_retired_or_given_members_shows_in_the_next_answer() {
        let mut m = made_up("cohorts-doors");
        m.list();
        m.settled();
        assert_eq!(m.reads(), 1);
        cohort::create(
            &mut m.registry,
            "second",
            "bo@lab",
            Some("made now"),
            "bo@lab",
        )
        .unwrap();
        let next = m.list();
        assert_eq!(next.as_array().unwrap().len(), 2, "{next}");
        let made = of(&next, "second");
        assert_eq!(made["owner"], "bo@lab");
        assert_eq!(made["description"], "made now");
        assert_eq!(made["subjects"], 0);
        assert_eq!(made["counting"], false, "{made}");
        assert_eq!(of(&next, "first")["counting"], true);
        m.settled();
        // renamed, described and retired: the row as it is now
        cohort::set(
            &mut m.registry,
            "first",
            &cohort::Change {
                name: Some("renamed"),
                owner: None,
                description: Some(Some("said now")),
                retired: Some(true),
            },
            "anna@lab",
        )
        .unwrap();
        let next = m.list();
        let renamed = of(&next, "renamed");
        assert_eq!(renamed["description"], "said now");
        assert!(renamed["retired_at"].is_string(), "{renamed}");
        assert!(
            next.as_array()
                .unwrap()
                .iter()
                .all(|c| c["name"] != "first")
        );
        m.settled();
        // members given at the door: the door reads the cohort again
        let before = m.reads();
        m.joined("second", &["sub-one", "sub-three"]);
        member_change(&mut m.registry, "second").unwrap();
        // one change costs one gathering, the next request's: the door's
        // own upkeep gathers nothing, which it would only throw away
        assert_eq!(m.reads(), before, "the door gathers nothing");
        let next = m.list();
        let given = of(&next, "second");
        assert_eq!(given["subjects"], 2, "{given}");
        assert_eq!(
            given["counting"], true,
            "the whole list is gathered again apart"
        );
        m.settled();
        assert_eq!(m.reads(), before + 1);
        let settled = m.list();
        assert_eq!(of(&settled, "second")["subjects"], 2);
        assert_eq!(of(&settled, "second")["counting"], false);
        // and a list never gathered before the door: the door gathers it
        let mut n = made_up("cohorts-doors-new");
        n.joined("first", &["sub-two"]);
        member_change(&mut n.registry, "first").unwrap();
        assert_eq!(of(&n.list(), "first")["subjects"], 1);
        n.settled();
    }

    /// 2026-11-12: a start of the engine answers the list the registry
    /// keeps and gathers nothing, and gathers apart where the registry
    /// moved while no engine looked.
    #[test]
    fn a_start_answers_the_registry_s_list_without_gathering() {
        let mut m = made_up("cohorts-start");
        m.joined("first", &["sub-one"]);
        let first = m.list();
        m.settled();
        assert_eq!(m.reads(), 1);
        let slot = slot_of(&m.registry);
        // another process over the same registry: nothing kept in memory
        KEPT.dropped(&slot);
        let mut registry = m.home.open().unwrap();
        let again = super::list(&mut registry).unwrap();
        assert_eq!(again, first);
        assert_eq!(m.reads(), 1, "the registry's list, nothing gathered");
        // the registry moved while no engine ran: the kept list at once,
        // one gathering apart
        m.joined("first", &["sub-two"]);
        KEPT.dropped(&slot);
        let stale = super::list(&mut registry).unwrap();
        assert_eq!(of(&stale, "first")["subjects"], 1);
        assert_eq!(of(&stale, "first")["counting"], true);
        m.settled();
        assert_eq!(m.reads(), 2);
        assert_eq!(of(&m.list(), "first")["subjects"], 2);
    }

    fn stamp_round(store: &mut Store) {
        let (first, busy) = stamp(store, &[], &[]).unwrap();
        assert!(first.starts_with("counting 1|"), "{first}");
        assert!(!busy);
        let subject = store.qualified("subject");
        let member = store.qualified("cohort_member");
        let cohort = store.qualified("cohort");
        store
            .batch(&format!(
                "INSERT INTO {subject} (code, created_at) VALUES ('sub-one', '2026-11-12T00:00:00Z'); \
                 INSERT INTO {cohort} (name, owner, created_at) \
                 VALUES ('first', 'anna@lab', '2026-11-12T00:00:00Z'); \
                 INSERT INTO {member} (cohort_id, subject_id, joined_at, source) \
                 SELECT c.id, s.id, '2026-11-12T00:00:00Z', 'manual' FROM {cohort} c, {subject} s"
            ))
            .unwrap();
        let cohorts = cohort::all(store).unwrap();
        let (joined, _) = stamp(store, &cohorts, &[]).unwrap();
        assert_ne!(joined, first);
        store
            .batch(&format!(
                "UPDATE {member} SET left_at = '2026-11-12T01:00:00Z'"
            ))
            .unwrap();
        let (left, _) = stamp(store, &cohorts, &[]).unwrap();
        assert_ne!(left, joined, "a member who left moves the stamp");
        assert_eq!(stamp(store, &cohorts, &[]).unwrap().0, left);
    }

    #[test]
    fn the_stamp_reads_on_sqlite() {
        let mut store = Store::sqlite_in_memory().unwrap();
        nils_registry::migrate::migrate(&mut store, nils_registry::migrate::Kind::Registry)
            .unwrap();
        stamp_round(&mut store);
    }

    #[test]
    fn the_stamp_reads_on_postgres() {
        let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
            .ok()
            .filter(|d| !d.is_empty())
        else {
            eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
            return;
        };
        const SCHEMA: &str = "nils_cohorts_stamp";
        let mut store = Store::connect_postgres(&dsn, SCHEMA).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; CREATE SCHEMA {SCHEMA}"
            ))
            .unwrap();
        nils_registry::migrate::migrate(&mut store, nils_registry::migrate::Kind::Registry)
            .unwrap();
        stamp_round(&mut store);
        store
            .batch(&format!("DROP SCHEMA IF EXISTS {SCHEMA} CASCADE"))
            .unwrap();
    }
}
