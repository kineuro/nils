// SPDX-License-Identifier: AGPL-3.0-only

//! Wave 7a (record 55 H2): a dataset's scans a page at a time, for the
//! list under its card on the Data page. A dataset's scans are the stacks
//! its digests created first, the ones the sources door counts as its
//! `totals.stacks`, so the list and the count agree. They are read straight
//! from the registry, never through a cohort: a dataset that feeds none
//! still lists its scans.
//!
//! The dataset viewer (2026-10-09) asks the same door of a cohort, whose
//! scans are every stack of a current member from any dataset
//! ([`crate::viewer::Scope`]), and narrows a page to one visit: a session
//! the cache holds, or the studies of a visit it does not hold yet.
//!
//! The page is keyset paged: sorted by subject code, then day, then stack
//! id, and `after` is the last stack id of the page before, so the cursor
//! carries nothing but a technical key. A stack of a sample sealed now is
//! never listed to a caller who does not read sealed stacks (record 48).
//!
//! Record 55 K7: below detail quasi a quasi-identifying field is answered
//! as its shape, as the value sampler shows it: the subject's code, the day
//! and a session label the scheme makes from a date. The subject's and the
//! session's ids are technical keys and are answered as they are, so a
//! caller at plain still groups by subject and session. The series
//! description is technical, as every sequence name is (record 55 K7, the
//! ruling of 2026-10-01).

use std::collections::{BTreeSet, HashMap};

use nils_registry::Registry;
use nils_registry::place::{self, Place, Role};
use nils_registry::session::{Naming, Scheme};
use serde_json::{Value, json};

use crate::grants::{Access, Detail};
use crate::serve::Reply;

/// A page's size when none is asked for.
pub(crate) const SCANS_PAGE: usize = 50;
/// The most a page holds.
pub(crate) const SCANS_MOST: usize = 200;

fn failed(e: impl std::fmt::Display) -> Reply {
    Reply::error(500, e.to_string())
}

/// The shape of a value as the value sampler shows it: digits become `9`,
/// letters `a` or `A`, the rest stays, capped at 40 characters with `~`.
/// The same function as `nils_ask`'s sampler (record 55 K7).
pub(crate) fn shape(v: &str) -> String {
    let mut out = String::new();
    for c in v.chars().take(40) {
        out.push(match c {
            '0'..='9' => '9',
            'a'..='z' => 'a',
            'A'..='Z' => 'A',
            other => other,
        });
    }
    if v.chars().count() > 40 {
        out.push('~');
    }
    out
}

/// The dataset a door names: an active source place, by its name or its id.
pub(crate) fn dataset_named(registry: &mut Registry, name: &str) -> Result<Place, Reply> {
    let store = registry.store();
    let mut found = place::by_name(store, name).map_err(failed)?;
    if found.is_none()
        && let Ok(id) = name.parse::<i64>()
    {
        found = place::show(store, id).map_err(failed)?;
    }
    match found {
        Some(p) if p.role == Role::Source && p.retired_at.is_none() => Ok(p),
        _ => Err(Reply::error(404, format!("no dataset named {name}"))),
    }
}

/// What a page of scans is narrowed to: one visit of the dataset viewer,
/// a session the cache holds or the studies of a visit it does not hold.
pub(crate) enum Visit {
    Session(i64),
    Studies(Vec<i64>),
}

/// One page of a scope's scans, `{scope, dataset, dataset_id | cohort,
/// cohort_id, detail, total, count, scans, next}`: a dataset's (the stacks
/// its digests created first) or a cohort's (every stack of a current
/// member, from any dataset), all of them or one visit's.
pub(crate) fn page(
    registry: &mut Registry,
    access: &Access,
    scope: &crate::viewer::Scope,
    visit: Option<&Visit>,
    limit: usize,
    after: Option<i64>,
) -> Result<Value, Reply> {
    let quasi = access.detail >= Detail::Quasi;
    let empty = |total: i64| {
        let mut doc = json!({
            "scope": scope.as_json(),
            "detail": access.detail.name(),
            "total": total,
            "count": 0,
            "scans": [],
            "next": null,
        });
        let (name, id) = match scope {
            crate::viewer::Scope::Dataset { .. } => ("dataset", "dataset_id"),
            crate::viewer::Scope::Cohort(_) => ("cohort", "cohort_id"),
        };
        doc[name] = json!(scope.name());
        doc[id] = json!(scope.id());
        doc
    };
    let store = registry.store();
    // an identified dataset nothing has read yet (only its originals, no
    // digest of its pseudonymised tree) holds no scans: an empty list
    let Some(mut filter) = scope.holds(store, access, "st", "b", "se") else {
        return Ok(empty(0));
    };
    match visit {
        Some(Visit::Session(id)) => filter.push_str(&format!(
            " AND se.study_id IN (SELECT scs.study_id FROM {} scs WHERE scs.session_id = {id})",
            store.qualified("session_cache_study")
        )),
        Some(Visit::Studies(ids)) => filter.push_str(&format!(
            " AND se.study_id IN ({})",
            ids.iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )),
        None => {}
    }
    let d = store.dialect();
    let [
        stack,
        batch,
        series,
        study,
        subject,
        fp,
        cached,
        cache,
        labels,
    ] = [
        "stack",
        "ingest_batch",
        "series",
        "study",
        "subject",
        "stack_fingerprint",
        "session_cache_study",
        "session_cache",
        "session_label",
    ]
    .map(|t| store.qualified(t));
    let study_t = nils_registry::schema::table("study");
    let date = |alias: &str, column: &str| {
        d.text_of_qualified(Some(alias), study_t.column(column).expect("study column"))
    };
    let day = |a: &str| {
        format!(
            "COALESCE({}, {})",
            date(a, "date_filled"),
            date(a, "study_date")
        )
    };
    let from = |s: &str, b: &str, se: &str, sy: &str, su: &str| {
        format!(
            "{stack} {s} JOIN {batch} {b} ON {b}.id = {s}.first_batch_id \
             JOIN {series} {se} ON {se}.id = {s}.series_id \
             JOIN {study} {sy} ON {sy}.id = {se}.study_id \
             JOIN {subject} {su} ON {su}.id = {se}.subject_id"
        )
    };
    let base = from("st", "b", "se", "sy", "su");
    let total = store
        .query(&format!("SELECT COUNT(*) FROM {base} WHERE {filter}"), &[])
        .map_err(failed)?[0]
        .int(0)
        .map_err(failed)?;
    // the order, as columns: the code, the day (none sorts first) and the id
    let order = |su: &str, sy: &str, st: &str| {
        format!(
            "COALESCE({su}.code, ''), COALESCE({}, ''), {st}.id",
            day(sy)
        )
    };
    let mut paged = filter.clone();
    if let Some(after) = after {
        // the cursor is a stack id; where it sits in the order is read
        // here, so no date or code ever travels in a cursor
        let known = store
            .query(
                &format!("SELECT COUNT(*) FROM {base} WHERE {filter} AND st.id = {after}"),
                &[],
            )
            .map_err(failed)?[0]
            .int(0)
            .map_err(failed)?;
        if known == 0 {
            return Err(Reply::error(
                400,
                format!("after names no scan of {} this caller lists", scope.name()),
            ));
        }
        paged.push_str(&format!(
            " AND ({}) > (SELECT {} FROM {} WHERE st2.id = {after})",
            order("su", "sy", "st"),
            order("su2", "sy2", "st2"),
            from("st2", "b2", "se2", "sy2", "su2"),
        ));
    }
    let rows = store
        .query(
            &format!(
                "SELECT st.id, su.id, su.code, sy.id, {}, f.text_series_description, \
                 st.orientation, st.n_instances FROM {base} \
                 LEFT JOIN {fp} f ON f.stack_id = st.id \
                 WHERE {paged} ORDER BY {} LIMIT {}",
                day("sy"),
                order("su", "sy", "st"),
                limit + 1
            ),
            &[],
        )
        .map_err(failed)?;
    struct Row {
        stack: i64,
        subject_id: i64,
        code: Option<String>,
        study: i64,
        day: Option<String>,
        name: Option<String>,
        orientation: Option<String>,
        images: Option<i64>,
    }
    let mut read = Vec::with_capacity(rows.len());
    for r in &rows {
        read.push(Row {
            stack: r.int(0).map_err(failed)?,
            subject_id: r.int(1).map_err(failed)?,
            code: r.opt_text(2).map_err(failed)?.map(str::to_string),
            study: r.int(3).map_err(failed)?,
            day: r.opt_text(4).map_err(failed)?.map(str::to_string),
            name: r.opt_text(5).map_err(failed)?.map(str::to_string),
            orientation: r.opt_text(6).map_err(failed)?.map(str::to_string),
            images: r.opt_int(7).map_err(failed)?,
        });
    }
    let next = if read.len() > limit {
        read.truncate(limit);
        read.last().map(|r| r.stack)
    } else {
        None
    };

    // the sessions of the page's studies, under the window the cache was
    // built with and the default scheme's labels, read once for the page
    let scheme = Scheme::default();
    let window = nils_registry::cohort::built_window(store)
        .map_err(failed)?
        .unwrap_or(scheme.window_days);
    let digest = scheme.digest();
    let dated_labels = matches!(scheme.naming, Naming::Date);
    let studies: BTreeSet<i64> = read.iter().map(|r| r.study).collect();
    let mut sessions: HashMap<(i64, i64), (i64, Option<String>)> = HashMap::new();
    if !studies.is_empty() {
        let list = studies
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let found = store
            .query(
                &format!(
                    "SELECT scs.study_id, sc.subject_id, sc.id, sl.label FROM {cached} scs \
                     JOIN {cache} sc ON sc.id = scs.session_id \
                     LEFT JOIN {labels} sl ON sl.session_id = sc.id AND sl.scheme_digest = '{digest}' \
                     WHERE scs.window_days = {window} AND scs.study_id IN ({list}) \
                     ORDER BY sc.id"
                ),
                &[],
            )
            .map_err(failed)?;
        for r in &found {
            let at = (r.int(0).map_err(failed)?, r.int(1).map_err(failed)?);
            sessions.entry(at).or_insert((
                r.int(2).map_err(failed)?,
                r.opt_text(3).map_err(failed)?.map(str::to_string),
            ));
        }
    }

    let shaped = |v: Option<String>, hide: bool| match v {
        Some(v) if hide => Value::from(shape(&v)),
        Some(v) => Value::from(v),
        None => Value::Null,
    };
    let scans: Vec<Value> = read
        .into_iter()
        .map(|r| {
            let session = sessions.get(&(r.study, r.subject_id)).cloned();
            json!({
                "stack": r.stack,
                "subject": {"id": r.subject_id, "code": shaped(r.code, !quasi)},
                "session": session.map(|(id, label)| json!({
                    "id": id,
                    "label": shaped(label, !quasi && dated_labels),
                })),
                // the study's id, a technical key: the desk keys a visit
                // the cache holds no session for by it
                "study": r.study,
                "series_description": r.name,
                "orientation": r.orientation,
                "images": r.images,
                "day": shaped(r.day, !quasi),
            })
        })
        .collect();
    let mut doc = empty(total);
    doc["count"] = json!(scans.len());
    doc["scans"] = json!(scans);
    doc["next"] = json!(next);
    Ok(doc)
}

/// Wave 7a, the dataset view (2026-10-09): what each scan of a page is
/// called and what NILS says it is, so the desk can show a dataset as a
/// tree of subject, session, datatype and scan. Each scan gains `name`, the
/// descriptive name (v0's grammar, as a release in the descriptive layout
/// builds it); `bids`, its BIDS name in the full style without the `sub-`
/// and `ses-` entities the tree already says, or null where the standard
/// has none; `datatype`, `anat`, `dwi`, `func`, `perf`, `fmap` or `other`;
/// `folder`, the descriptive layout's folder (`anat/SyMRI`, `localizer`,
/// ...); `axes`, every decided axis as stored; `series_number`, the
/// order the scanner acquired it in; and `questions`, the kinds of its open
/// review questions, as a page with pictures gives them. Each name is built from
/// the scan's own facts: a release also separates two scans of a session
/// that build one name, which a page, holding part of a session, cannot.
/// None of it is quasi-identifying.
pub(crate) fn with_names(
    registry: &mut Registry,
    pack: Option<&nils_pack::Pack>,
    doc: &mut Value,
) -> Result<(), Reply> {
    let stacks: Vec<i64> = doc["scans"]
        .as_array()
        .map(|a| a.iter().filter_map(|s| s["stack"].as_i64()).collect())
        .unwrap_or_default();
    let mut names = nils_release::run::scan_names(registry.store(), pack, &stacks)
        .map_err(|e| Reply::error(500, e.to_string()))?;
    // the open questions too, so the tree marks the scans that need a look
    // without asking for pictures, and what the sort noted
    let questions = open_questions(registry.store(), &stacks).map_err(failed)?;
    let notes = noted(registry.store(), &stacks).map_err(failed)?;
    if let Some(scans) = doc["scans"].as_array_mut() {
        for scan in scans {
            let stack = scan["stack"].as_i64().unwrap_or_default();
            scan["questions"] = json!(questions.get(&stack).cloned().unwrap_or_default());
            scan["notes"] = json!(notes.get(&stack).cloned().unwrap_or_default());
            match names.remove(&stack) {
                Some(n) => {
                    scan["name"] = json!(n.name);
                    scan["bids"] = json!(n.bids);
                    scan["datatype"] = json!(n.datatype);
                    scan["folder"] = json!(n.folder);
                    scan["axes"] = json!(n.axes);
                    scan["series_number"] = json!(n.series_number);
                }
                None => {
                    scan["name"] = Value::Null;
                    scan["bids"] = Value::Null;
                    scan["datatype"] = json!("other");
                    scan["folder"] = json!("misc");
                    scan["axes"] = json!({});
                    scan["series_number"] = Value::Null;
                }
            }
        }
    }
    Ok(())
}

/// How long a page of scans waits for the stills of scans with no preview.
pub const STILLS_BUDGET: std::time::Duration = std::time::Duration::from_millis(100);

/// Record 55 H2 (E2): a page of scans with what the grid draws, so a page
/// of fifty is one request. Each scan gains `questions`, the kinds of the
/// open review questions on its stack (a classifier's grouped question
/// under each of its members, and a question about the stack alone), so
/// the desk can mark the scans that need a look; and `picture`, its own
/// middle plane from the preview its sort made, as a data URL
/// `{data, width, height, digest, held, partial}`. A scan with no preview
/// yet has its middle plane decoded from its one file (`partial` true) as
/// long as the page's budget lasts, and null after (decoded on, and its
/// preview made on a thread of the engine, never through the queue, so the
/// page after has it). Pictures are pixels: they are shown as the
/// instance doors show them, with query:see at detail quasi, one
/// `instance.open` audit row a stack in the window, and the band held
/// below detail sensitive where the stack carries burned-in annotation;
/// below that every `picture` is null and `pictures.shown` false with why.
pub(crate) fn with_pictures(
    registry: &mut Registry,
    caller: &crate::serve::Caller,
    home: &nils_registry::home::Home,
    doc: &mut Value,
) -> Result<(), Reply> {
    let stacks: Vec<i64> = doc["scans"]
        .as_array()
        .map(|a| a.iter().filter_map(|s| s["stack"].as_i64()).collect())
        .unwrap_or_default();
    let questions = open_questions(registry.store(), &stacks).map_err(failed)?;
    let notes = noted(registry.store(), &stacks).map_err(failed)?;
    let access = &caller.access;
    let why = if !access.holds("query:see") {
        Some("pictures are pixels, which query:see opens")
    } else if access.detail < Detail::Quasi {
        Some("pictures are quasi-identifying; detail quasi opens them")
    } else {
        None
    };
    let working = crate::pyramid::working_place_cached(registry.store()).ok();
    let mut pictures: HashMap<i64, Value> = HashMap::new();
    let mut missing = Vec::new();
    let mut partial = 0usize;
    let sensitive = access.detail >= Detail::Sensitive;
    if let (None, Some(w)) = (why, &working) {
        let root = std::path::Path::new(&w.path);
        let plain: Vec<std::path::PathBuf> = stacks
            .iter()
            .map(|s| crate::preview::path(root, *s, false))
            .collect();
        let mut opened = crate::preview::opened_many(&plain);
        // the band held below detail sensitive: the held file of a stack
        // that carries burned-in annotation
        for (i, o) in opened.iter_mut().enumerate() {
            if let Some(found) = o
                && found.header.burned_in
                && !sensitive
            {
                *o = crate::preview::opened(&crate::preview::path(root, stacks[i], true))
                    .ok()
                    .flatten();
            }
        }
        for (stack, o) in stacks.iter().zip(opened) {
            match o.as_ref().and_then(|o| o.middle("axial").map(|m| (o, m))) {
                Some((o, (jpeg, b))) => {
                    crate::pyramid::note_open(registry, caller, *stack, None, 0, "list")
                        .map_err(|e| Reply::error(500, e))?;
                    pictures.insert(
                        *stack,
                        json!({
                            "data": crate::preview::data_url(jpeg),
                            "width": b.width,
                            "height": b.height,
                            "digest": o.header.digest,
                            "held": o.header.held,
                            "partial": false,
                        }),
                    );
                }
                None => missing.push(*stack),
            }
        }
        // a scan with no preview yet: its middle plane from one file, as
        // many as are decoded within the page's budget; the rest are
        // missing for now, and decoded on for the page asked next
        let stills = crate::preview::stills_within(registry.store(), &missing, STILLS_BUDGET);
        for stack in &missing {
            if let Some(s) = stills.get(stack) {
                crate::pyramid::note_open(registry, caller, *stack, None, 0, "list")
                    .map_err(|e| Reply::error(500, e))?;
                let (jpeg, held) = s.picture(sensitive);
                pictures.insert(
                    *stack,
                    json!({
                        "data": crate::preview::data_url(jpeg),
                        "width": s.width,
                        "height": s.height,
                        "digest": s.digest,
                        "held": held,
                        "partial": true,
                    }),
                );
                partial += 1;
            }
        }
        crate::preview::warm(home, root, &missing);
        missing.retain(|s| !stills.contains_key(s));
    }
    if let Some(scans) = doc["scans"].as_array_mut() {
        for scan in scans {
            let stack = scan["stack"].as_i64().unwrap_or_default();
            scan["questions"] = json!(questions.get(&stack).cloned().unwrap_or_default());
            scan["notes"] = json!(notes.get(&stack).cloned().unwrap_or_default());
            scan["picture"] = pictures.remove(&stack).unwrap_or(Value::Null);
        }
    }
    doc["pictures"] = json!({
        "shown": why.is_none() && working.is_some(),
        "why": why.or(working.is_none().then_some("no working place is bound, where previews are kept")),
        "missing": missing.len(),
        "partial": partial,
        "place": working.map(|w| w.name),
    });
    Ok(())
}

/// Record 55 H3 (2026-10-09): what the sort noted on each of `stacks` that
/// is information and not a question, as `{kind, ...}` objects: the split
/// note, `{kind: "split:one_image_per_stack", value, stacks_in_series,
/// n_instances}`, where the stack's series was split into stacks of one
/// image each.
fn noted(
    store: &mut nils_registry::Store,
    stacks: &[i64],
) -> Result<HashMap<i64, Vec<Value>>, nils_registry::store::Error> {
    let mut out: HashMap<i64, Vec<Value>> = HashMap::new();
    if stacks.is_empty() {
        return Ok(out);
    }
    let list = stacks
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let t = nils_registry::schema::table("classification");
    let notes = store.dialect().text_of(t.column("notes").expect("notes"));
    for r in store.query(
        &format!(
            "SELECT stack_id, {notes} FROM {} WHERE stack_id IN ({list}) AND notes IS NOT NULL",
            store.qualified("classification")
        ),
        &[],
    )? {
        let doc: Value = r
            .opt_text(1)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null);
        if doc["split"].is_object() {
            out.entry(r.int(0)?).or_default().push(doc["split"].clone());
        }
    }
    Ok(out)
}

/// The kinds of the review questions that still wait for a person (open
/// or staged, the one rule every look is counted by,
/// [`crate::certainty::OPEN`]) on each of `stacks`, sorted and each once:
/// grouped questions through their members, and questions about one stack
/// through their reference.
fn open_questions(
    store: &mut nils_registry::Store,
    stacks: &[i64],
) -> Result<HashMap<i64, BTreeSet<String>>, nils_registry::store::Error> {
    let mut out: HashMap<i64, BTreeSet<String>> = HashMap::new();
    if stacks.is_empty() {
        return Ok(out);
    }
    let list = stacks
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let items = store.qualified("review_item");
    let members = store.qualified("review_member");
    let open = crate::certainty::OPEN;
    for r in store.query(
        &format!(
            "SELECT m.stack_id, ri.kind FROM {members} m JOIN {items} ri ON ri.id = m.item_id \
             WHERE {open} AND m.stack_id IN ({list})"
        ),
        &[],
    )? {
        out.entry(r.int(0)?)
            .or_default()
            .insert(r.text(1)?.to_string());
    }
    // a question about one stack names it in its reference, which the two
    // backends spell apart as text: read and matched here, not in SQL
    let wanted: BTreeSet<i64> = stacks.iter().copied().collect();
    for (kind, stack) in crate::certainty::stack_scoped(store)? {
        if wanted.contains(&stack) {
            out.entry(stack).or_default().insert(kind);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::shape;

    #[test]
    fn a_shape_keeps_the_form_and_hides_the_value() {
        assert_eq!(shape("2026-01-02"), "9999-99-99");
        assert_eq!(shape("sub-0a1F"), "aaa-9a9A");
        assert_eq!(shape(&"x".repeat(41)), format!("{}~", "a".repeat(40)));
    }
}
