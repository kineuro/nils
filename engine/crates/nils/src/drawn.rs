// SPDX-License-Identifier: AGPL-3.0-only

//! Record 55 H3, nothing typed by hand: the create door draws the items of a
//! pair, an anchored and an A/B campaign from saved selections, where the
//! keyboard reads them from files (`nils campaign pair | anchored | ab`).
//! Both make the campaign through one function of each kind
//! ([`crate::pair::make_pairs`], [`crate::anchored::make_anchored`],
//! [`crate::ab::make_ab`]), so a door and the keyboard check, count, draw
//! and audit alike; `POST /api/campaigns/preview` answers the same counts
//! and writes nothing but the frozen selections' handles.
//!
//! - **Pairs** (`source.pair`): with `with`, each stack of `selection` beside
//!   each stack of `with` in the same session; without it, every two stacks
//!   of `selection` in one session. Never a stack with itself, a pair once.
//!   A session is one the session cache holds (the visits door's meaning);
//!   a stack whose study is in no cached session pairs within its study.
//! - **Anchored items** (`source.anchored`): each candidate of `selection`
//!   with its pre anchor from `pre` and its post anchor from `post`, each
//!   the nearest by series number in the candidate's own session (ties to
//!   the lower stack id), never the candidate itself; where its session
//!   holds none, the same subject's nearest session's by day, flagged as of
//!   another session. A candidate with no anchor of its subject is left out
//!   and counted.
//! - **A/B items** (`source.ab`): the stacks of `selection`, read by the
//!   voters whose readings ride in the body (one document per stack, keyed
//!   by its id, as the keyboard reads one file per stack) and by the rules
//!   when `rules` is true. The rules are a system: on a stack of a sample
//!   sealed now they vote only for a caller who holds sealed:see, which is
//!   audited as `sealed.read`; anyone else is refused (403, `sealed: true`).

use std::collections::{BTreeMap, BTreeSet};

use nils_ask::ast::Grain;
use nils_registry::Registry;
use nils_registry::anchored::Named;
use nils_registry::day::Day;
use nils_registry::store::{Error as StoreError, Store};
use serde_json::{Map, Value, json};

use crate::serve::{Caller, Doors, Reply};

/// What went wrong in making a drawn campaign: the status a door answers
/// with, and the words.
pub(crate) type Refused = (u16, String);

/// The most pairs one pair campaign is drawn with.
pub(crate) const PAIRS_MOST: usize = 10_000;
/// The most items one anchored campaign is drawn with.
pub(crate) const ANCHORED_MOST: usize = 10_000;
/// The most readings the voters of one A/B campaign bring in a body.
pub(crate) const READINGS_MOST: usize = 50_000;

/// A refusal as the keyboard says it.
pub(crate) fn exit((status, words): Refused) -> crate::Exit {
    if status >= 500 {
        crate::fail(words)
    } else {
        crate::usage(words)
    }
}

/// A door's refusal as a [`Refused`].
pub(crate) fn refused(r: Reply) -> Refused {
    let words = r.body["error"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| r.body.to_string());
    (r.status, words)
}

fn reply((status, words): Refused) -> Reply {
    Reply::error(status, words)
}

fn failed(e: StoreError) -> Reply {
    Reply::error(500, e.to_string())
}

/// A pair or an anchored campaign to make, as the keyboard and the door
/// give it.
pub(crate) struct Making<'a> {
    pub name: &'a str,
    pub owner: &'a str,
    pub question: Value,
    pub seed: &'a str,
    pub raters: Vec<String>,
    pub raters_per_item: i64,
    pub lease_seconds: i64,
    pub served: Option<&'a nils_pack::Pack>,
    pub dry_run: bool,
    /// Counts the door adds to the campaign's own, what was left out.
    pub counts: Map<String, Value>,
}

/// Which of the drawn sources a body names, if any.
pub(crate) fn drawn_kind(doc: &Value) -> Option<&'static str> {
    let source = &doc["source"];
    ["pair", "anchored", "ab"]
        .into_iter()
        .find(|k| source[*k].is_object())
}

/// Where a stack sits: its subject, its study, the session the cache holds
/// it under (or none), its study's day and its series number.
#[derive(Debug, Clone)]
struct Sits {
    subject: i64,
    study: i64,
    session: Option<i64>,
    day: Option<Day>,
    series: Option<i64>,
}

impl Sits {
    /// The session a stack is drawn within: the cached one, else its study.
    fn group(&self) -> (i64, Option<i64>, i64) {
        match self.session {
            Some(s) => (self.subject, Some(s), 0),
            None => (self.subject, None, self.study),
        }
    }
}

fn list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Where each of these stacks sits.
fn sits(store: &mut Store, stacks: &[i64]) -> Result<BTreeMap<i64, Sits>, StoreError> {
    let w = crate::viewer::window(store)?;
    let day = crate::viewer::study_day(store, "sy");
    let mut out = BTreeMap::new();
    for chunk in stacks.chunks(500) {
        let sql = format!(
            "SELECT st.id, se.subject_id, se.study_id, scs.session_id, {day}, fp.series_number \
             FROM {} st JOIN {} se ON se.id = st.series_id JOIN {} sy ON sy.id = se.study_id \
             LEFT JOIN {} scs ON scs.study_id = sy.id AND scs.window_days = {w} \
             LEFT JOIN {} fp ON fp.stack_id = st.id WHERE st.id IN ({})",
            store.qualified("stack"),
            store.qualified("series"),
            store.qualified("study"),
            store.qualified("session_cache_study"),
            store.qualified("stack_fingerprint"),
            list(chunk)
        );
        for r in store.query(&sql, &[])? {
            out.insert(
                r.int(0)?,
                Sits {
                    subject: r.int(1)?,
                    study: r.int(2)?,
                    session: r.opt_int(3)?,
                    day: r.opt_text(4)?.and_then(Day::parse),
                    series: r.opt_int(5)?,
                },
            );
        }
    }
    Ok(out)
}

/// The pairs a selection draws, with or without a second one: each pair
/// once, its two stacks of one session. Answers the pairs and how many
/// stacks of either selection are in none.
fn pairs_of(
    at: &BTreeMap<i64, Sits>,
    one: &[i64],
    with: Option<&[i64]>,
) -> (Vec<(i64, i64)>, usize) {
    type Group = (i64, Option<i64>, i64);
    let grouped = |ids: &[i64]| -> BTreeMap<Group, BTreeSet<i64>> {
        let mut g: BTreeMap<Group, BTreeSet<i64>> = BTreeMap::new();
        for id in ids {
            if let Some(s) = at.get(id) {
                g.entry(s.group()).or_default().insert(*id);
            }
        }
        g
    };
    let a = grouped(one);
    let mut pairs: BTreeSet<(i64, i64)> = BTreeSet::new();
    match with {
        None => {
            for ids in a.values() {
                let ids: Vec<i64> = ids.iter().copied().collect();
                for (i, x) in ids.iter().enumerate() {
                    for y in &ids[i + 1..] {
                        pairs.insert((*x, *y));
                    }
                }
            }
        }
        Some(with) => {
            let b = grouped(with);
            for (g, xs) in &a {
                let Some(ys) = b.get(g) else { continue };
                for x in xs {
                    for y in ys {
                        if x != y {
                            pairs.insert((*x.min(y), *x.max(y)));
                        }
                    }
                }
            }
        }
    }
    let paired: BTreeSet<i64> = pairs.iter().flat_map(|&(x, y)| [x, y]).collect();
    let all: BTreeSet<i64> = one
        .iter()
        .chain(with.unwrap_or_default())
        .copied()
        .collect();
    let left = all.difference(&paired).count();
    (pairs.into_iter().collect(), left)
}

/// The nearest anchor of a candidate among these: its own session's first,
/// by series number; else the subject's nearest session's by day. Never the
/// candidate, never `not`. Answers the anchor and whether it is of another
/// session.
fn nearest(
    at: &BTreeMap<i64, Sits>,
    candidate: i64,
    among: &BTreeSet<i64>,
    not: Option<i64>,
) -> Option<(i64, bool)> {
    let c = at.get(&candidate)?;
    among
        .iter()
        .filter(|a| **a != candidate && Some(**a) != not)
        .filter_map(|a| at.get(a).map(|s| (*a, s)))
        .filter(|(_, s)| s.subject == c.subject)
        .map(|(a, s)| {
            let other = s.group() != c.group();
            let days = if other {
                match (c.day, s.day) {
                    (Some(x), Some(y)) => x.days_to(y).abs(),
                    _ => i64::MAX,
                }
            } else {
                0
            };
            let series = match (c.series, s.series) {
                (Some(x), Some(y)) => (x - y).abs(),
                _ => i64::MAX,
            };
            ((other, days, s.group(), series, a), (a, other))
        })
        .min_by(|x, y| x.0.cmp(&y.0))
        .map(|(_, found)| found)
}

/// The anchored items a candidates' selection draws with its anchors'
/// selections: each candidate's nearest pre and post. Answers the items,
/// whether each anchor is of another session, and how many candidates were
/// left out with no anchor of their subject.
fn anchored_of(
    at: &BTreeMap<i64, Sits>,
    candidates: &[i64],
    pre: &[i64],
    post: &[i64],
) -> (Vec<Named>, BTreeMap<i64, (bool, bool)>, usize) {
    let (pre, post): (BTreeSet<i64>, BTreeSet<i64>) = (
        pre.iter().copied().collect(),
        post.iter().copied().collect(),
    );
    let mut items = Vec::new();
    let mut other = BTreeMap::new();
    let mut left = 0usize;
    for c in candidates.iter().copied().collect::<BTreeSet<i64>>() {
        let Some((p, p_other)) = nearest(at, c, &pre, None) else {
            left += 1;
            continue;
        };
        let Some((q, q_other)) = nearest(at, c, &post, Some(p)) else {
            left += 1;
            continue;
        };
        items.push(Named {
            candidate: c,
            pre: p,
            post: q,
        });
        other.insert(c, (p_other, q_other));
    }
    (items, other, left)
}

fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str, Reply> {
    v[key]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Reply::error(400, format!("{key}: a saved selection, as name@version")))
}

/// The stacks a selection holds, frozen now under the caller's grants.
fn frozen(
    doors: &Doors,
    registry: &mut Registry,
    ask: &mut crate::ask_doors::AskState,
    caller: &Caller,
    spec: &str,
) -> Result<(i64, Vec<i64>), Reply> {
    let (h, keys) =
        crate::campaigns::freeze_at_door(doors, registry, ask, caller, spec, Grain::Stack)?;
    Ok((h, keys.into_iter().map(|(k, _)| k).collect()))
}

/// `POST /api/campaigns` and its preview for a drawn source: the campaign
/// made (as the create door answers, with the kind's own counts under its
/// name), or with `dry_run` the counts alone, written nowhere.
pub(crate) fn door(
    doors: &Doors,
    registry: &mut Registry,
    ask: &mut crate::ask_doors::AskState,
    caller: &Caller,
    doc: &Value,
    dry_run: bool,
) -> Result<Value, Reply> {
    let kind = drawn_kind(doc).ok_or_else(|| Reply::error(400, "source: pair, anchored or ab"))?;
    let name = doc["name"].as_str().map(str::trim).unwrap_or_default();
    if !dry_run && name.is_empty() {
        return Err(Reply::error(400, "name: what the campaign is called"));
    }
    let asked = doc["question"]["kind"].as_str();
    let fits = match kind {
        "ab" => matches!(asked, None | Some("axes")),
        k => asked.is_none() || asked == Some(k),
    };
    if !fits {
        return Err(Reply::error(
            400,
            format!(
                "question.kind: a source of {kind} asks {}",
                if kind == "ab" { "axes" } else { kind }
            ),
        ));
    }
    let served = doors
        .pack_dir
        .as_ref()
        .and_then(|dir| nils_pack::load(&dir.join(&doors.ask_pack), None).ok());
    let source = &doc["source"][kind];
    let seed = source["seed"].as_str().unwrap_or_default();
    let (mut doc_out, made) = match kind {
        "pair" | "anchored" => {
            let mut question = if doc["question"].is_object() {
                doc["question"].clone()
            } else {
                json!({})
            };
            question["kind"] = json!(kind);
            for (key, default) in [
                ("axis", nils_registry::pair::AXIS),
                ("post", nils_registry::pair::POST),
                ("pre", nils_registry::pair::PRE),
            ] {
                if question[key].is_null() {
                    question[key] = json!(default);
                }
            }
            let m = |counts: Map<String, Value>| Making {
                name,
                owner: &caller.principal,
                question: question.clone(),
                seed,
                raters: crate::campaigns::strings_of(&doc["raters"]),
                raters_per_item: doc["raters_per_item"].as_i64().unwrap_or(1),
                lease_seconds: doc["lease_seconds"].as_i64().unwrap_or(3600),
                served: served.as_ref(),
                dry_run,
                counts,
            };
            if kind == "pair" {
                let (_, one) = frozen(doors, registry, ask, caller, text(source, "selection")?)?;
                let with = match source["with"].as_str().filter(|s| !s.trim().is_empty()) {
                    Some(spec) => Some(frozen(doors, registry, ask, caller, spec)?.1),
                    None => None,
                };
                let all: Vec<i64> = one.iter().chain(with.iter().flatten()).copied().collect();
                let at = sits(registry.store(), &all).map_err(failed)?;
                let (pairs, left) = pairs_of(&at, &one, with.as_deref());
                let counts = || {
                    let mut c = Map::new();
                    c.insert("left_out".into(), json!(left));
                    c
                };
                if pairs.is_empty() || pairs.len() > PAIRS_MOST {
                    let why = if pairs.is_empty() {
                        "the selections draw no pair: no two of their stacks are of one session"
                            .to_string()
                    } else {
                        format!(
                            "the selections draw {} pairs; a pair campaign holds at most {PAIRS_MOST}",
                            pairs.len()
                        )
                    };
                    let stacks = all.iter().collect::<BTreeSet<_>>().len();
                    let counts = json!({"pairs": pairs.len(), "stacks": stacks, "left_out": left});
                    if dry_run {
                        return Ok(
                            json!({"kind": kind, "name": name, "counts": counts, "dry_run": true, "refusal": why}),
                        );
                    }
                    let mut r = Reply::error(400, why);
                    r.body["counts"] = counts;
                    return Err(r);
                }
                crate::pair::make_pairs(registry, &m(counts()), &pairs).map_err(reply)?
            } else {
                let (_, candidates) =
                    frozen(doors, registry, ask, caller, text(source, "selection")?)?;
                let (_, pre) = frozen(doors, registry, ask, caller, text(source, "pre")?)?;
                let (_, post) = frozen(doors, registry, ask, caller, text(source, "post")?)?;
                let all: Vec<i64> = candidates
                    .iter()
                    .chain(&pre)
                    .chain(&post)
                    .copied()
                    .collect();
                let at = sits(registry.store(), &all).map_err(failed)?;
                let (items, other, left) = anchored_of(&at, &candidates, &pre, &post);
                let mut counts = Map::new();
                counts.insert("left_out".into(), json!(left));
                if items.is_empty() || items.len() > ANCHORED_MOST {
                    let why = if items.is_empty() {
                        "no candidate has a pre and a post anchor of its subject".to_string()
                    } else {
                        format!(
                            "the selections draw {} items; an anchored campaign holds at most {ANCHORED_MOST}",
                            items.len()
                        )
                    };
                    let counts = json!({"items": items.len(), "candidates": candidates.len(), "left_out": left});
                    if dry_run {
                        return Ok(
                            json!({"kind": kind, "name": name, "counts": counts, "dry_run": true, "refusal": why}),
                        );
                    }
                    let mut r = Reply::error(400, why);
                    r.body["counts"] = counts;
                    return Err(r);
                }
                crate::anchored::make_anchored(registry, &m(counts), &items, Some(&other))
                    .map_err(reply)?
            }
        }
        _ => ab_door(
            registry,
            doors,
            ask,
            caller,
            doc,
            name,
            served.as_ref(),
            dry_run,
        )?,
    };
    doc_out["kind"] = json!(kind);
    match made {
        None => Ok(doc_out),
        Some(made) => {
            let mut v = crate::campaigns::shown(registry.store(), &made)?;
            v["pictures"] = doc_out["pictures"].clone();
            v[kind] = doc_out;
            Ok(v)
        }
    }
}

/// The A/B source at the door: the selection frozen, the voters' readings
/// read from the body and the rules as a voter where asked.
#[allow(clippy::too_many_arguments)]
fn ab_door(
    registry: &mut Registry,
    doors: &Doors,
    ask: &mut crate::ask_doors::AskState,
    caller: &Caller,
    doc: &Value,
    name: &str,
    served: Option<&nils_pack::Pack>,
    dry_run: bool,
) -> Result<(Value, Option<nils_registry::campaign::Campaign>), Reply> {
    let source = &doc["source"]["ab"];
    let rules = match &source["rules"] {
        Value::Null => false,
        Value::Bool(b) => *b,
        _ => return Err(Reply::error(400, "rules: true or false")),
    };
    let voters_doc = match &source["voters"] {
        Value::Null => Map::new(),
        Value::Object(m) => m.clone(),
        _ => {
            return Err(Reply::error(
                400,
                "voters: each voter's readings, {name: {stack id: answer}}",
            ));
        }
    };
    if voters_doc.is_empty() && !rules {
        return Err(Reply::error(
            400,
            "an A/B campaign needs a voter: rules, or voters' readings",
        ));
    }
    let readings: usize = voters_doc
        .values()
        .map(|v| v.as_object().map_or(0, Map::len))
        .sum();
    if readings > READINGS_MOST {
        return Err(Reply::error(
            413,
            format!("voters: {readings} readings; one body brings at most {READINGS_MOST}"),
        ));
    }
    let num = |key: &str| source[key].as_f64();
    let d = crate::ab::drawing(
        doc["question"]["axes"]
            .as_array()
            .map(|_| crate::campaigns::strings_of(&doc["question"]["axes"])),
        doc["question"]["derive"]
            .as_array()
            .map(|_| crate::campaigns::strings_of(&doc["question"]["derive"])),
        source["seed"].as_str().unwrap_or_default(),
        num("audit_share").unwrap_or(0.1),
        num("audit_cells").map_or(150, |n| n.max(0.0) as usize),
        source["localizers"].as_str().unwrap_or("with"),
        source["localizer"]
            .as_str()
            .unwrap_or("provenance=Localizer"),
        if source["localizer_asks"].is_array() {
            crate::campaigns::strings_of(&source["localizer_asks"])
        } else {
            vec!["provenance".into(), "body_part".into()]
        },
        served,
    )
    .map_err(reply)?;
    let (axes, constraints) = (d.axes(), d.constraints());
    let selection = text(source, "selection")?;
    let (handle, stacks) = frozen(doors, registry, ask, caller, selection)?;
    let mut voters = Vec::new();
    let mut reads = Map::new();
    for (voter, readings) in &voters_doc {
        let voter = voter.trim();
        if voter.is_empty() {
            return Err(Reply::error(400, "voters: a voter has a name"));
        }
        if voter == "rules" && rules {
            return Err(Reply::error(
                400,
                "voters: the name rules is the rules' own",
            ));
        }
        let Some(readings) = readings.as_object() else {
            return Err(Reply::error(
                400,
                format!("voters.{voter}: its readings, {{stack id: answer}}"),
            ));
        };
        let docs = readings
            .iter()
            .map(|(k, v)| (k.trim().parse::<i64>().ok(), v.clone()));
        let (v, read) = crate::ab::voter_from(voter, docs, &axes, &constraints, served);
        reads.insert(voter.to_string(), read.as_json());
        voters.push(v);
    }
    if rules {
        // what a system said of a stack sealed now is withheld (record 48)
        let sealed = crate::sealed::now(registry.store(), &stacks).map_err(failed)?;
        if !sealed.is_empty() {
            if !crate::sealed::reads(&caller.access) {
                let mut r = Reply::error(
                    403,
                    format!(
                        "{} of the stacks the rules would vote on are of a sample sealed for certification, and what a system said of them is withheld until a certificate unseals it (record 48); a holder of sealed:see may make it, audited as sealed.read",
                        sealed.len()
                    ),
                );
                r.body["sealed"] = json!(true);
                return Err(r);
            }
            nils_registry::audit::record(
                registry,
                &nils_registry::audit::Entry {
                    principal: &caller.principal,
                    action: nils_registry::audit::Action::SealedRead,
                    scope: json!({"door": "POST /api/campaigns", "voter": "rules", "selection": selection}),
                    policy: None,
                    job_id: None,
                    details: Some(json!({"grant": crate::grants::UNSEALED, "stacks": sealed.len()})),
                },
            )
            .map_err(|e| Reply::error(500, e.to_string()))?;
        }
        let (v, read) =
            crate::ab::rules_voter(registry.store(), &stacks, &axes, &constraints, served)
                .map_err(failed)?;
        reads.insert("rules".into(), read.as_json());
        voters.push(v);
    }
    let closes_into = doc["closes_into"].as_str().unwrap_or("none");
    crate::ab::make_ab(
        registry,
        &crate::ab::Ab {
            name,
            owner: &caller.principal,
            selection,
            handle,
            stacks: &stacks,
            voters: &voters,
            reads,
            drawing: &d,
            served,
            raters: crate::campaigns::strings_of(&doc["raters"]),
            closes_into,
            lease_seconds: doc["lease_seconds"].as_i64().unwrap_or(3600),
            dry_run,
        },
    )
    .map_err(reply)
}

/// What the capabilities say of the drawn sources.
pub(crate) fn capability() -> Value {
    json!({
        "draws": ["pair", "anchored", "ab"],
        "pairs_max": PAIRS_MOST,
        "anchored_max": ANCHORED_MOST,
        "ab_readings_max": READINGS_MOST,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stack, its subject, study, session, day and series number.
    type Row<'a> = (i64, i64, i64, Option<i64>, &'a str, i64);

    fn at(rows: &[Row<'_>]) -> BTreeMap<i64, Sits> {
        rows.iter()
            .map(|&(id, subject, study, session, day, series)| {
                (
                    id,
                    Sits {
                        subject,
                        study,
                        session,
                        day: Day::parse(day),
                        series: Some(series),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn pairs_are_of_one_session_and_drawn_once() {
        // subject 1: session 10 holds 1, 2, 3; subject 2: study 50 (no session) holds 4, 5
        let at = at(&[
            (1, 1, 7, Some(10), "2020-01-01", 1),
            (2, 1, 7, Some(10), "2020-01-01", 2),
            (3, 1, 8, Some(10), "2020-01-02", 3),
            (4, 2, 50, None, "2021-01-01", 1),
            (5, 2, 50, None, "2021-01-01", 2),
            (6, 3, 60, None, "2021-01-01", 1),
        ]);
        let (pairs, left) = pairs_of(&at, &[1, 2, 3, 4, 5, 6], None);
        assert_eq!(pairs, vec![(1, 2), (1, 3), (2, 3), (4, 5)]);
        assert_eq!(left, 1);
        let (pairs, left) = pairs_of(&at, &[1, 4], Some(&[1, 2, 5, 6]));
        assert_eq!(pairs, vec![(1, 2), (4, 5)]);
        assert_eq!(left, 1);
    }

    #[test]
    fn an_anchor_is_its_sessions_nearest_else_the_nearest_sessions() {
        let at = at(&[
            (1, 1, 7, Some(10), "2020-01-01", 5),
            (2, 1, 7, Some(10), "2020-01-01", 3),
            (3, 1, 7, Some(10), "2020-01-01", 7),
            (4, 1, 8, Some(11), "2020-06-01", 7),
            (5, 1, 9, Some(12), "2022-06-01", 7),
            (6, 2, 20, Some(20), "2020-01-01", 1),
        ]);
        let (items, other, left) = anchored_of(&at, &[1, 6], &[2, 3, 1], &[4, 5]);
        assert_eq!(left, 1);
        assert_eq!(
            items,
            vec![Named {
                candidate: 1,
                pre: 2,
                post: 4
            }]
        );
        assert_eq!(other[&1], (false, true));
        // a tie in series distance goes to the lower stack
        let (items, _, _) = anchored_of(&at, &[1], &[3, 2], &[3, 2]);
        assert_eq!((items[0].pre, items[0].post), (2, 3));
    }
}
