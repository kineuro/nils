// SPDX-License-Identifier: AGPL-3.0-only

//! Wave 5 section 12.4, slice A5: the dependency door. The closure of an
//! adoption, a pack bump or an erasure: the stacks that move, the review
//! items that open and close, and the handles and releases that stop being
//! reproducible. Counts and ids, never a row of a person. The confirm
//! panels read it before an operator commits; the adopt door records what
//! it computed and invalidates the handles it names (section 12.8).

use std::collections::BTreeSet;

use nils_registry::schema::Type;
use nils_registry::store::Param;
use nils_registry::{Registry, Store};
use serde_json::{Value, json};

use crate::serve::{Doors, Reply};

/// The kinds the door serves. A rule change (section 12.4) waits for the
/// identity rules to become registry objects; it is not a kind yet.
pub(crate) const KINDS: &[&str] = &["overlay", "pack", "subject", "stack"];

const SAMPLE: usize = 50;

/// The closure of one change.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct Closure {
    pub kind: String,
    pub id: String,
    /// The stacks that move: how many, and up to fifty of them.
    pub stacks: Stacks,
    pub review: Review,
    pub handles: Vec<HandleRef>,
    pub releases: Vec<ReleaseRef>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct Stacks {
    pub count: usize,
    pub sample: Vec<i64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct Review {
    pub opens: i64,
    pub closes: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct HandleRef {
    pub handle: i64,
    pub name: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct ReleaseRef {
    pub release: i64,
    pub name: String,
    pub version: String,
}

impl Closure {
    /// The counts an audit row keeps.
    pub(crate) fn counts(&self) -> Value {
        json!({
            "stacks": self.stacks.count,
            "review": {"opens": self.review.opens, "closes": self.review.closes},
            "handles": self.handles.len(),
            "releases": self.releases.len(),
        })
    }
}

pub(crate) enum Outcome {
    Closure(Closure),
    NoKind,
    NoObject,
}

pub(crate) fn of(
    doors: &Doors,
    registry: &mut Registry,
    kind: &str,
    id: &str,
) -> Result<Outcome, Reply> {
    let (stacks, review, extra): (Vec<i64>, Review, Vec<HandleRef>) = match kind {
        "overlay" => {
            let oid: i64 = match id.parse() {
                Ok(n) => n,
                Err(_) => return Ok(Outcome::NoObject),
            };
            let Some(o) = nils_registry::overlay::show(registry.store(), oid)? else {
                return Ok(Outcome::NoObject);
            };
            let (moved, review) = overlay_moves(doors, registry, &o)?;
            (moved, review, Vec::new())
        }
        "pack" => {
            let (name, version) = pack_target(doors, id)?;
            let moved = stacks_behind(registry.store(), &name, &version)?;
            if moved.is_empty() && !pack_known(registry.store(), &name)? {
                return Ok(Outcome::NoObject);
            }
            let closes = open_items_on(registry.store(), &moved)?;
            // handles run under another version of the pack stop reproducing
            let mut extra = Vec::new();
            for h in nils_ask::handle::list(registry.store(), false)? {
                if h.pack_version.as_deref().is_some_and(|v| v != version) {
                    extra.push(HandleRef {
                        handle: h.id,
                        name: h.name.clone(),
                        reason: format!(
                            "run under pack version {}, and {name} moves to {version}",
                            h.pack_version.as_deref().unwrap_or("?")
                        ),
                    });
                }
            }
            (moved, Review { opens: 0, closes }, extra)
        }
        "subject" => {
            let sid: i64 = match id.parse() {
                Ok(n) => n,
                Err(_) => return Ok(Outcome::NoObject),
            };
            if !exists(registry.store(), "subject", sid)? {
                return Ok(Outcome::NoObject);
            }
            let moved = stacks_of_subject(registry.store(), sid)?;
            let closes = open_items_on(registry.store(), &moved)?;
            (moved, Review { opens: 0, closes }, Vec::new())
        }
        "stack" => {
            let stid: i64 = match id.parse() {
                Ok(n) => n,
                Err(_) => return Ok(Outcome::NoObject),
            };
            if !exists(registry.store(), "stack", stid)? {
                return Ok(Outcome::NoObject);
            }
            let closes = open_items_on(registry.store(), &[stid])?;
            (vec![stid], Review { opens: 0, closes }, Vec::new())
        }
        _ => return Ok(Outcome::NoKind),
    };
    let subjects = subjects_of(registry.store(), &stacks)?;
    let mut handles = handles_reading(registry.store(), &stacks, &subjects, kind)?;
    for h in extra {
        if !handles.iter().any(|x| x.handle == h.handle) {
            handles.push(h);
        }
    }
    handles.sort_by_key(|h| h.handle);
    let releases = releases_over(registry.store(), &stacks)?;
    Ok(Outcome::Closure(Closure {
        kind: kind.to_string(),
        id: id.to_string(),
        stacks: Stacks {
            count: stacks.len(),
            sample: stacks.iter().take(SAMPLE).copied().collect(),
        },
        review,
        handles,
        releases,
    }))
}

/// Record 56 §6.5: the closure of a change whose stacks are known, an
/// adopted scoped rule change's: the stacks its scope holds, the open
/// questions on them, and the handles and releases that read them.
pub(crate) fn of_stacks(
    registry: &mut Registry,
    kind: &str,
    id: &str,
    stacks: Vec<i64>,
) -> Result<Closure, Reply> {
    let closes = open_items_on(registry.store(), &stacks)?;
    let subjects = subjects_of(registry.store(), &stacks)?;
    let handles = handles_reading(registry.store(), &stacks, &subjects, kind)?;
    let releases = releases_over(registry.store(), &stacks)?;
    Ok(Closure {
        kind: kind.to_string(),
        id: id.to_string(),
        stacks: Stacks {
            count: stacks.len(),
            sample: stacks.iter().take(SAMPLE).copied().collect(),
        },
        review: Review { opens: 0, closes },
        handles,
        releases,
    })
}

/// Wave 5 §12.8: the handles a closure names stop reproducing. One row
/// each, once; the ids written are returned.
pub(crate) fn invalidate(
    registry: &mut Registry,
    closure: &Closure,
    reason: &str,
    by: &str,
) -> Result<Vec<i64>, Reply> {
    let mut written = Vec::new();
    for h in &closure.handles {
        if nils_ask::handle::invalidate(
            registry.store(),
            h.handle,
            reason,
            &closure.kind,
            &closure.id,
            by,
        )? {
            written.push(h.handle);
        }
    }
    Ok(written)
}

fn overlay_moves(
    doors: &Doors,
    registry: &mut Registry,
    o: &nils_registry::overlay::Overlay,
) -> Result<(Vec<i64>, Review), Reply> {
    let overlay = nils_pack::Overlay::parse("overlay", &o.document.to_string())
        .map_err(|e| Reply::error(500, format!("overlay {} does not parse: {e}", o.id)))?;
    let scope_text = o.scope["over"]
        .as_str()
        .ok_or_else(|| Reply::error(500, format!("overlay {} has no scope", o.id)))?;
    let scope = nils_classify::scope::Scope::parse(scope_text).map_err(|e| Reply::error(500, e))?;
    let dir = doors
        .pack_dir
        .as_ref()
        .map(|d| d.join(&overlay.pack))
        .filter(|d| d.join("pack.yml").is_file())
        .ok_or_else(|| {
            Reply::error(
                409,
                format!(
                    "the overlay amends {}, which this engine does not serve",
                    overlay.pack
                ),
            )
        })?;
    let (before, _) =
        nils_pack::load_judged(&dir, None).map_err(|e| Reply::error(500, e.to_string()))?;
    let (after, _) = nils_pack::load_judged(&dir, Some(&overlay))
        .map_err(|e| Reply::error(500, e.to_string()))?;
    let sample = nils_classify::rehearse::SAMPLE_MAX;
    let moved =
        nils_classify::rehearse::moved_stacks(registry.store(), &before, &after, &scope, sample)?;
    let tried = nils_classify::rehearse::run(
        registry.store(),
        &before,
        &after,
        &scope,
        sample,
        None,
        (0, None),
    )?;
    let review = Review {
        opens: tried["review_items"]["open"].as_i64().unwrap_or(0),
        closes: tried["review_items"]["close"].as_i64().unwrap_or(0),
    };
    Ok((moved, review))
}

/// `name` or `name@version`; without a version, the version this engine serves.
fn pack_target(doors: &Doors, id: &str) -> Result<(String, String), Reply> {
    let (name, version) = match id.split_once('@') {
        Some((n, v)) => (n.to_string(), Some(v.to_string())),
        None => (id.to_string(), None),
    };
    let version = match version {
        Some(v) => v,
        None => {
            let dir = doors
                .pack_dir
                .as_ref()
                .map(|d| d.join(&name))
                .filter(|d| d.join("pack.yml").is_file())
                .ok_or_else(|| {
                    Reply::error(
                        404,
                        format!("no pack {name} is served; name a version as {name}@<version>"),
                    )
                })?;
            nils_pack::load(&dir, None)
                .map_err(|e| Reply::error(500, e.to_string()))?
                .version
                .to_string()
        }
    };
    Ok((name, version))
}

fn exists(store: &mut Store, table: &str, id: i64) -> Result<bool, Reply> {
    let d = store.dialect();
    let sql = format!(
        "SELECT 1 FROM {} WHERE id = {}",
        store.qualified(table),
        d.param(1, Type::Int)
    );
    Ok(store.query_opt(&sql, &[Param::Int(id)])?.is_some())
}

fn pack_known(store: &mut Store, name: &str) -> Result<bool, Reply> {
    let d = store.dialect();
    let sql = format!(
        "SELECT 1 FROM {} WHERE pack = {} LIMIT 1",
        store.qualified("classification"),
        d.param(1, Type::Text)
    );
    Ok(store.query_opt(&sql, &[Param::from(name)])?.is_some())
}

/// The stacks whose latest classification by `name` is not `version`.
fn stacks_behind(store: &mut Store, name: &str, version: &str) -> Result<Vec<i64>, Reply> {
    let d = store.dialect();
    let c = store.qualified("classification");
    let sql = format!(
        "SELECT c.stack_id FROM {c} c WHERE c.id IN (SELECT MAX(id) FROM {c} WHERE pack = {} GROUP BY stack_id) \
         AND c.pack_version <> {} ORDER BY c.stack_id",
        d.param(1, Type::Text),
        d.param(2, Type::Text)
    );
    Ok(store
        .query(&sql, &[Param::from(name), Param::from(version)])?
        .iter()
        .map(|r| r.int(0))
        .collect::<Result<Vec<_>, _>>()?)
}

fn stacks_of_subject(store: &mut Store, subject: i64) -> Result<Vec<i64>, Reply> {
    let d = store.dialect();
    let sql = format!(
        "SELECT st.id FROM {} st JOIN {} se ON se.id = st.series_id WHERE se.subject_id = {} ORDER BY st.id",
        store.qualified("stack"),
        store.qualified("series"),
        d.param(1, Type::Int)
    );
    Ok(store
        .query(&sql, &[Param::Int(subject)])?
        .iter()
        .map(|r| r.int(0))
        .collect::<Result<Vec<_>, _>>()?)
}

/// `IN (...)` over a chunk of ids, as holes and params from `start`.
fn holes(store: &Store, ids: &[i64], start: usize) -> (String, Vec<Param>) {
    let d = store.dialect();
    let text: Vec<String> = (0..ids.len())
        .map(|i| d.param(start + i, Type::Int))
        .collect();
    (
        text.join(", "),
        ids.iter().map(|i| Param::Int(*i)).collect(),
    )
}

fn subjects_of(store: &mut Store, stacks: &[i64]) -> Result<BTreeSet<i64>, Reply> {
    let mut out = BTreeSet::new();
    for chunk in stacks.chunks(500) {
        let (list, params) = holes(store, chunk, 1);
        let sql = format!(
            "SELECT DISTINCT se.subject_id FROM {} st JOIN {} se ON se.id = st.series_id WHERE st.id IN ({list})",
            store.qualified("stack"),
            store.qualified("series")
        );
        for r in store.query(&sql, &params)? {
            out.insert(r.int(0)?);
        }
    }
    Ok(out)
}

/// The kept handles that read any of the stacks (a stack handle naming
/// one) or any of their subjects (a handle whose members belong to one).
fn handles_reading(
    store: &mut Store,
    stacks: &[i64],
    subjects: &BTreeSet<i64>,
    kind: &str,
) -> Result<Vec<HandleRef>, Reply> {
    let mut by_stack: BTreeSet<i64> = BTreeSet::new();
    for chunk in stacks.chunks(500) {
        let (list, params) = holes(store, chunk, 1);
        let sql = format!(
            "SELECT DISTINCT hm.handle_id FROM {} hm JOIN {} h ON h.id = hm.handle_id \
             WHERE h.grain = 'stack' AND h.withdrawn_at IS NULL AND hm.key IN ({list})",
            store.qualified("handle_member"),
            store.qualified("handle")
        );
        for r in store.query(&sql, &params)? {
            by_stack.insert(r.int(0)?);
        }
    }
    let mut by_subject: BTreeSet<i64> = BTreeSet::new();
    let subjects: Vec<i64> = subjects.iter().copied().collect();
    for chunk in subjects.chunks(500) {
        let (list, params) = holes(store, chunk, 1);
        let sql = format!(
            "SELECT DISTINCT hm.handle_id FROM {} hm JOIN {} h ON h.id = hm.handle_id \
             WHERE h.withdrawn_at IS NULL AND hm.subject_id IN ({list})",
            store.qualified("handle_member"),
            store.qualified("handle")
        );
        for r in store.query(&sql, &params)? {
            by_subject.insert(r.int(0)?);
        }
    }
    let already = nils_ask::handle::invalidated(store)?;
    let what = match kind {
        "overlay" => "stacks the overlay moves",
        "pack" => "stacks the pack bump reclassifies",
        "proposal" => "stacks the scoped rule change re-sorts",
        "subject" => "the subject erased",
        _ => "the stack erased",
    };
    let mut out = Vec::new();
    for id in by_stack.union(&by_subject) {
        if already.contains(id) {
            continue;
        }
        let Some(h) = nils_ask::handle::get(store, *id)? else {
            continue;
        };
        let reason = if by_stack.contains(id) {
            format!("names {what}")
        } else {
            format!("reads subjects of {what}")
        };
        out.push(HandleRef {
            handle: h.id,
            name: h.name,
            reason,
        });
    }
    Ok(out)
}

/// The finished versions that hold any of the stacks. A stack's current
/// row may name a release that stopped or died before it finished (Wave
/// 7b, the maintenance track): that is not a version, and a change's closure
/// does not list it.
fn releases_over(store: &mut Store, stacks: &[i64]) -> Result<Vec<ReleaseRef>, Reply> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for chunk in stacks.chunks(500) {
        let (list, params) = holes(store, chunk, 1);
        let sql = format!(
            "SELECT DISTINCT rel.id, rel.name, rel.version FROM {} rs JOIN {} rel ON rel.id = rs.release_id \
             WHERE rel.withdrawn_at IS NULL AND rel.finished_at IS NOT NULL \
             AND rs.stack_id IN ({list}) ORDER BY rel.id",
            store.qualified("release_stack"),
            store.qualified("release")
        );
        for r in store.query(&sql, &params)? {
            let id = r.int(0)?;
            if seen.insert(id) {
                out.push(ReleaseRef {
                    release: id,
                    name: r.text(1)?.to_string(),
                    version: r.text(2)?.to_string(),
                });
            }
        }
    }
    Ok(out)
}

/// Open review items holding any of the stacks: what an erasure or a bump closes.
fn open_items_on(store: &mut Store, stacks: &[i64]) -> Result<i64, Reply> {
    let mut items = BTreeSet::new();
    for chunk in stacks.chunks(500) {
        let (list, params) = holes(store, chunk, 1);
        let sql = format!(
            "SELECT DISTINCT i.id FROM {} i JOIN {} m ON m.item_id = i.id WHERE i.status = 'open' AND m.stack_id IN ({list})",
            store.qualified("review_item"),
            store.qualified("review_member")
        );
        for r in store.query(&sql, &params)? {
            items.insert(r.int(0)?);
        }
    }
    Ok(items.len() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nils_dicom::synth::TempDir;
    use nils_registry::home::Home;
    use nils_registry::schema::table;
    use nils_registry::store::Insert;

    fn registry(dir: &TempDir) -> (Home, Registry) {
        let home = Home::new(dir.path().join("registry"));
        home.keys(None).add("k", b"nils-fixture-key").unwrap();
        let registry = home
            .init(&nils_registry::InitOptions {
                backend: nils_registry::Backend::Sqlite,
                dsn: None,
                schema: None,
                scheme: nils_registry::pseudonym::Scheme::Blake2b32,
                key: "k".to_string(),
                display_length: nils_registry::pseudonym::DEFAULT_DISPLAY_LENGTH,
                session_scheme: None,
            })
            .unwrap();
        (home, registry)
    }

    /// One row of a table, the columns given set and every other required
    /// column filled with a stand-in of its type.
    fn stub(store: &mut Store, name: &str, set: &[(&str, Param)]) {
        let t = table(name);
        let mut columns: Vec<&str> = Vec::new();
        let mut values: Vec<Param> = Vec::new();
        for c in &t.columns {
            if let Some((_, v)) = set.iter().find(|(n, _)| *n == c.name) {
                columns.push(c.name);
                values.push(v.clone());
            } else if c.not_null && c.ty != Type::Id {
                columns.push(c.name);
                values.push(match c.ty {
                    Type::Int => Param::Int(0),
                    Type::Double => Param::Double(0.0),
                    Type::Bool => Param::Bool(false),
                    Type::Bytes => Param::Bytes(Vec::new()),
                    Type::Json => Param::from("{}"),
                    Type::Date => Param::from("2026-01-01"),
                    Type::Time => Param::from("00:00:00.000000"),
                    Type::Timestamp => Param::from("2026-01-01T00:00:00Z"),
                    Type::Id | Type::Text => Param::from("x"),
                });
            }
        }
        store.insert(&Insert::new(t, &columns), &[values]).unwrap();
    }

    /// Wave 7b (the maintenance track): a stack whose current row names a
    /// release that did not finish (stopped, or died) is held by no version:
    /// the closure of a change over it lists no release, and its timeline,
    /// and its subject's, say nothing of being released. A stack of the same
    /// subject that a finished version holds is listed as before.
    #[test]
    fn a_release_that_did_not_finish_holds_no_stack() {
        let dir = TempDir::new("depends-releases");
        let (_home, mut registry) = registry(&dir);
        let store = registry.store();
        let release = |finished: Option<&str>| {
            vec![
                ("name", Param::from("test")),
                ("version", Param::from("2026.01.01.1")),
                ("dataset_id", Param::Int(1)),
                ("started_at", Param::from("2026-01-01T10:00:00Z")),
                ("finished_at", finished.map_or(Param::Null, Param::from)),
                ("error", Param::from("stopped after 1 of 2 stacks")),
            ]
        };
        // id 1 finished, id 2 stopped: stack 1 was last written by the
        // stopped one, stack 2 by the finished one
        stub(store, "release", &release(Some("2026-01-01T10:05:00Z")));
        stub(store, "release", &release(None));
        stub(store, "subject", &[("id", Param::Int(1))]);
        stub(
            store,
            "series",
            &[("id", Param::Int(1)), ("subject_id", Param::Int(1))],
        );
        for stack in [1, 2] {
            stub(
                store,
                "stack",
                &[
                    ("id", Param::Int(stack)),
                    ("series_id", Param::Int(1)),
                    ("stack_index", Param::Int(stack)),
                    ("stack_key", Param::Text(format!("k{stack}"))),
                ],
            );
        }
        stub(
            store,
            "release_stack",
            &[
                ("dataset_id", Param::Int(1)),
                ("stack_id", Param::Int(1)),
                ("release_id", Param::Int(2)),
            ],
        );
        stub(
            store,
            "release_stack",
            &[
                ("dataset_id", Param::Int(1)),
                ("stack_id", Param::Int(2)),
                ("release_id", Param::Int(1)),
            ],
        );

        let over = releases_over(store, &[1]).unwrap_or_else(|_| panic!("a store failure"));
        assert!(over.is_empty(), "{over:?}");
        let over = releases_over(store, &[1, 2]).unwrap_or_else(|_| panic!("a store failure"));
        assert_eq!(over.len(), 1, "{over:?}");
        assert_eq!(over[0].release, 1);

        let released = |registry: &mut Registry, kind: &str, id: i64| -> Vec<String> {
            match crate::timeline::of(registry, kind, id).unwrap() {
                crate::timeline::Outcome::Events(events) => events
                    .into_iter()
                    .filter(|e| e.kind == "released")
                    .map(|e| e.summary)
                    .collect(),
                _ => panic!("the timeline of {kind} {id} has no events"),
            }
        };
        assert!(released(&mut registry, "stack", 1).is_empty());
        assert_eq!(
            released(&mut registry, "stack", 2),
            vec!["released in test 2026.01.01.1 as x".to_string()]
        );
        assert_eq!(
            released(&mut registry, "subject", 1),
            vec!["1 stacks released in test 2026.01.01.1".to_string()]
        );
    }
}
