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

fn releases_over(store: &mut Store, stacks: &[i64]) -> Result<Vec<ReleaseRef>, Reply> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for chunk in stacks.chunks(500) {
        let (list, params) = holes(store, chunk, 1);
        let sql = format!(
            "SELECT DISTINCT rel.id, rel.name, rel.version FROM {} rs JOIN {} rel ON rel.id = rs.release_id \
             WHERE rel.withdrawn_at IS NULL AND rs.stack_id IN ({list}) ORDER BY rel.id",
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
