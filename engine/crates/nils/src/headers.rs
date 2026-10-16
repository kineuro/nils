// SPDX-License-Identifier: AGPL-3.0-only

//! The damaged-header flag on a dataset's card (record 56 §3; record 55,
//! ruling 31 (c)): set at the end of every sort, for each dataset holding a
//! scan the sort judged, from three signs read with the sort's own pack.
//!
//! - **Nameless**: of the dataset's MR scans, the share that carry neither a
//!   series description nor a protocol name.
//! - **Unknown words**: of its named MR scans, the share none of whose words
//!   the pack knows ([`nils_pack::words`]): names in another language, an
//!   exporter's placeholder, another site's abbreviations.
//! - **Unseen maker**: of its MR scans, the share by a maker the pack does
//!   not name as one its rules were built on and no other dataset of the
//!   registry held an MR scan of before this dataset's first scan of it came
//!   in, or that name no maker. A maker is seen before, not seen elsewhere:
//!   the first dataset of a new maker keeps its sign when a second comes.
//!
//! The pack gives each its threshold (`review.damaged_headers`, pack
//! contract 11), so a later change is a rules release and no engine
//! release; a dataset is flagged when one share reaches its threshold. A
//! pack before contract 11 gives none: the nameless share flags at
//! [`NAMELESS_DEFAULT`], as it did before, and the other two are read and
//! shown and flag nothing.
//!
//! What the signs read is kept on the dataset with the thresholds and the
//! pack that read them, so the Data card says why. A person sets or clears
//! the flag with a why, and that word wins over the signs until the person
//! lets them decide again. A dataset no sort has read since the signs were
//! kept is read when asked, and kept at its next sort. A root, whose every
//! folder is a dataset of its own, is no dataset: its datasets are flagged
//! each by their own signs, and a root only by a person.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use nils_pack::Pack;
use nils_pack::words::{Vocabulary, maker_of};
use nils_registry::place::{self, Place, Role};
use nils_registry::round;
use nils_registry::schema::Type;
use nils_registry::store::{Error as StoreError, Param, Store};
use serde_json::{Value, json};

/// The share of nameless MR scans that flags a dataset when its pack states
/// no thresholds (a pack before contract 11): the number the flag was built
/// with (record 56 §3, most of a dataset's scans). A named constant, used
/// only for such a pack; the MRI pack states its own.
pub(crate) const NAMELESS_DEFAULT: f64 = 0.5;

/// The pack the signs are read with where no sort hands one over (a door,
/// the gate after a run): the deployment's pack of that name.
pub(crate) fn served(pack_dir: Option<&std::path::Path>, name: &str) -> Option<Arc<Pack>> {
    crate::reader::served_pack(pack_dir, name)
}

/// The pack name the gate reads the signs with where it is told none: the
/// engine's own default pack, as `nils classify` and the doors default to.
pub(crate) const DEFAULT_PACK: &str = "mri";

/// The thresholds in force: the pack's, or for a pack that states none the
/// nameless share at the engine's default.
struct Thresholds {
    nameless: Option<f64>,
    unknown_words: Option<f64>,
    unseen_maker: Option<f64>,
    makers: Vec<String>,
    words: Option<String>,
    from: &'static str,
}

impl Thresholds {
    fn of(pack: Option<&Pack>) -> Thresholds {
        match pack.and_then(|p| p.review.damaged_headers.as_ref()) {
            Some(d) => Thresholds {
                nameless: d.nameless,
                unknown_words: d.unknown_words,
                unseen_maker: d.unseen_maker,
                makers: d.makers.clone(),
                words: d.words.clone(),
                from: "pack",
            },
            None => Thresholds {
                nameless: Some(NAMELESS_DEFAULT),
                unknown_words: None,
                unseen_maker: None,
                makers: Vec::new(),
                words: None,
                from: "engine",
            },
        }
    }
}

fn list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// A share as the card says it: whole percent from 10 % and for a whole
/// number, one decimal below 10 % otherwise.
fn pct(share: f64) -> String {
    let p = share * 100.0;
    let tenths = (p * 10.0).round();
    if p >= 10.0 || tenths % 10.0 == 0.0 {
        format!("{p:.0} %")
    } else {
        format!("{:.1} %", tenths / 10.0)
    }
}

/// Whether a share reaches its threshold, of a population that is there.
fn reaches(share: f64, threshold: Option<f64>, of: i64) -> bool {
    of > 0 && threshold.is_some_and(|t| share >= t)
}

/// The makers of the MR scans the other datasets hold, each with the first
/// of those scans the registry took in (the lowest stack id: ids are given
/// in the order scans come in): every source dataset but this one (roots
/// are no datasets), less the sources this one holds.
fn others_makers(
    store: &mut Store,
    mine: &[i64],
    modality: &str,
) -> Result<BTreeMap<String, i64>, StoreError> {
    let mine: BTreeSet<i64> = mine.iter().copied().collect();
    let mut others: BTreeSet<i64> = BTreeSet::new();
    for d in place::dataset_sources(store)? {
        others.extend(d.sources.into_iter().filter(|s| !mine.contains(s)));
    }
    let mut out: BTreeMap<String, i64> = BTreeMap::new();
    if others.is_empty() {
        return Ok(out);
    }
    let others: Vec<i64> = others.into_iter().collect();
    let holds = crate::operations::held_by(store, "x", &list(&others));
    let sql = format!(
        "SELECT COALESCE(f.manufacturer, ''), MIN(x.id) FROM {} x JOIN {} f ON f.stack_id = x.id \
         WHERE {holds} AND UPPER(COALESCE(f.modality, '')) = {} GROUP BY 1",
        store.qualified("stack"),
        store.qualified("stack_fingerprint"),
        store.dialect().param(1, Type::Text)
    );
    for r in store.query(&sql, &[Param::from(modality.to_string())])? {
        if let Some(m) = maker_of(r.text(0)?) {
            let first = r.int(1)?;
            out.entry(m)
                .and_modify(|f| *f = (*f).min(first))
                .or_insert(first);
        }
    }
    Ok(out)
}

/// What a dataset's signs read now, with `pack` (none: the nameless share
/// alone at the engine's default, and no words read): the counts and shares
/// of each sign, the threshold it is held to, whether it flags, and in words
/// why the dataset is flagged or not. `damaged` is the signs' answer; a
/// person's word is not read here.
pub(crate) fn read(
    store: &mut Store,
    sources: &[i64],
    pack: Option<&Pack>,
) -> Result<Value, StoreError> {
    let t = Thresholds::of(pack);
    let modality = pack.map_or_else(|| "MR".to_string(), |p| p.modality.to_uppercase());
    let vocabulary = pack.and_then(|p| Vocabulary::of(p, t.words.as_deref()).ok());
    let (mut scans, mut nameless, mut named, mut unread) = (0i64, 0i64, 0i64, 0i64);
    // a maker as a person names it, or "" for none: its scans, and the
    // first of them the registry took in
    let mut makers: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    if !sources.is_empty() {
        let holds = crate::operations::held_by(store, "x", &list(sources));
        // each distinct pair of names and maker once, with its scans: a
        // dataset of half a million scans writes some tens of thousands
        let sql = format!(
            "SELECT COALESCE(f.text_series_description, ''), COALESCE(f.text_protocol_name, ''), \
             COALESCE(f.manufacturer, ''), COUNT(*), MIN(x.id) FROM {} x JOIN {} f ON f.stack_id = x.id \
             WHERE {holds} AND UPPER(COALESCE(f.modality, '')) = {} GROUP BY 1, 2, 3",
            store.qualified("stack"),
            store.qualified("stack_fingerprint"),
            store.dialect().param(1, Type::Text)
        );
        for r in store.query(&sql, &[Param::from(modality.clone())])? {
            let (description, protocol, maker, n) = (r.text(0)?, r.text(1)?, r.text(2)?, r.int(3)?);
            let first = r.int(4)?;
            scans += n;
            if description.trim().is_empty() && protocol.trim().is_empty() {
                nameless += n;
            }
            if let Some(v) = &vocabulary {
                let read = v.read(description, protocol, maker);
                if read.words > 0 {
                    named += n;
                    if read.unread() {
                        unread += n;
                    }
                }
            }
            let (count, earliest) = makers
                .entry(maker_of(maker).unwrap_or_default())
                .or_insert((0, first));
            *count += n;
            *earliest = (*earliest).min(first);
        }
    }
    // seen before: a maker the rules were built on, or one another dataset
    // held a scan of before this dataset's first scan of it came in, so the
    // first dataset of a new maker keeps its sign when a second one comes
    let seen = others_makers(store, sources, &modality)?;
    let built_on: BTreeSet<String> = t.makers.iter().map(|m| m.trim().to_lowercase()).collect();
    let mut unseen = 0i64;
    let mut unseen_by: BTreeMap<String, i64> = BTreeMap::new();
    for (m, (n, first)) in &makers {
        let before = seen.get(m).is_some_and(|other| other < first);
        if m.is_empty() || (!built_on.contains(&m.to_lowercase()) && !before) {
            unseen += n;
            let name = if m.is_empty() { "(none)" } else { m.as_str() };
            unseen_by.insert(name.to_string(), *n);
        }
    }
    let share = |k: i64, of: i64| if of > 0 { k as f64 / of as f64 } else { 0.0 };
    let nameless_share = share(nameless, scans);
    let unknown_share = share(unread, named);
    let unseen_share = share(unseen, scans);
    let nameless_flags = reaches(nameless_share, t.nameless, scans);
    let unknown_flags = vocabulary.is_some() && reaches(unknown_share, t.unknown_words, named);
    let unseen_flags = reaches(unseen_share, t.unseen_maker, scans);
    let damaged = nameless_flags || unknown_flags || unseen_flags;

    let mut said: Vec<String> = Vec::new();
    if nameless_flags {
        said.push(format!(
            "{} of its MR scans carry neither a series description nor a protocol name (flags at {})",
            pct(nameless_share),
            pct(t.nameless.unwrap_or_default())
        ));
    }
    if unknown_flags {
        said.push(format!(
            "{} of its named MR scans carry no word the rules know (flags at {})",
            pct(unknown_share),
            pct(t.unknown_words.unwrap_or_default())
        ));
    }
    if unseen_flags {
        said.push(format!(
            "{} of its MR scans are by a maker the rules were not built on and no dataset brought in before it holds: {} (flags at {})",
            pct(unseen_share),
            unseen_by.keys().cloned().collect::<Vec<_>>().join(", "),
            pct(t.unseen_maker.unwrap_or_default())
        ));
    }
    let why = if damaged {
        said.join("; ")
    } else if scans == 0 {
        "it holds no MR scan".to_string()
    } else {
        "none of its signs reaches its threshold".to_string()
    };
    let mut rule: Vec<String> = Vec::new();
    if let Some(x) = t.nameless {
        rule.push(format!(
            "{} of its MR scans carry neither a series description nor a protocol name",
            pct(x)
        ));
    }
    if let Some(x) = t.unknown_words {
        rule.push(format!(
            "{} of its named MR scans carry no word the rules know",
            pct(x)
        ));
    }
    if let Some(x) = t.unseen_maker {
        rule.push(format!(
            "{} of its MR scans are by a maker the rules were not built on and no dataset brought in before it holds",
            pct(x)
        ));
    }
    let rule = match rule.len() {
        0 => "the signs flag nothing: the pack gives no threshold".to_string(),
        1 => format!("flagged by its signs where at least {}", rule[0]),
        _ => {
            let last = rule.pop().unwrap_or_default();
            format!(
                "flagged by its signs where at least {}, or {}",
                rule.join(", "),
                last
            )
        }
    };
    let words = match &vocabulary {
        Some(v) => json!({
            "named": named, "unread": unread, "share": unknown_share,
            "threshold": t.unknown_words, "flags": unknown_flags, "text": v.text,
        }),
        None => json!({
            "named": Value::Null, "unread": Value::Null, "share": Value::Null,
            "threshold": t.unknown_words, "flags": false,
            "not_read": "no pack to read the words with",
        }),
    };
    let by_maker: BTreeMap<String, i64> = makers
        .into_iter()
        .map(|(m, (n, _))| {
            (
                if m.is_empty() {
                    "(none)".to_string()
                } else {
                    m
                },
                n,
            )
        })
        .collect();
    Ok(json!({
        // as the flag's first form gave them, for a reader from before
        "scans": scans, "no_names": nameless, "share": nameless_share,
        "damaged": damaged, "why": why, "rule": rule,
        "nameless": {
            "scans": nameless, "share": nameless_share,
            "threshold": t.nameless, "flags": nameless_flags,
        },
        "unknown_words": words,
        "unseen_maker": {
            "scans": unseen, "share": unseen_share, "threshold": t.unseen_maker,
            "flags": unseen_flags, "makers": unseen_by, "built_on": t.makers,
        },
        "makers": by_maker,
        "thresholds": t.from,
        "pack": pack.map(Pack::id),
        "at": nils_registry::time::now_iso(),
    }))
}

/// A dataset's flag, as its card and the gate read it: a person's word
/// where one stands, else what its signs read at its last sort, else what
/// they read now with `pack`.
pub(crate) fn flag_of(
    store: &mut Store,
    place: &Place,
    sources: &[i64],
    pack: Option<&Pack>,
) -> Result<Value, StoreError> {
    let person = round::headers_flag(store, place.id)?;
    let root = place::is_root(&place.dataset);
    let signs = if root {
        json!({
            "scans": 0, "no_names": 0, "share": 0.0, "damaged": false, "read": "never",
            "why": "a root is no dataset: each of its datasets is flagged by its own signs",
        })
    } else {
        match round::headers_signs(store, place.id)? {
            Some(kept) => kept,
            None => {
                let mut now = read(store, sources, pack)?;
                now["read"] = json!(
                    "now, when asked: no sort has read them since they are kept, and the next sort keeps them"
                );
                now
            }
        }
    };
    let damaged = match &person {
        Some(f) => f["damaged"].as_bool().unwrap_or(false),
        None => signs["damaged"].as_bool().unwrap_or(false),
    };
    Ok(json!({
        "dataset": place.name, "damaged": damaged,
        "by": if person.is_some() { "person" } else { "signs" },
        "flag": person, "signs": signs,
        "gate": if damaged {
            "the full gate: System 1 must agree with the rules on each scan above its thresholds"
        } else {
            "the rules decide; Review holds only the axes that matter they left empty"
        },
    }))
}

/// Record 55, ruling 31 (c): the flag set at ingest. At the end of a sort,
/// each dataset holding a scan the sort judged (job `job`) has its signs
/// read with the sort's pack and kept; the flag follows them unless a
/// person's word stands. What was done is the answer, for the sort's
/// result; a dataset whose signs could not be read is said there and never
/// fails the sort.
pub(crate) fn after_sort(store: &mut Store, pack: &Pack, job: i64) -> Value {
    let mut read_now = Vec::new();
    let mut failed = Vec::new();
    // the flag says where System 1's gate runs, and System 1 reads MR scans:
    // a sort by a pack of another modality leaves the MR signs as they are
    if !pack.modality.eq_ignore_ascii_case("MR") {
        return json!({"datasets": [], "failed": [], "why": format!("the pack {} sorts {}, and the signs are read of MR scans", pack.id(), pack.modality)});
    }
    let places = match place::active(store) {
        Ok(p) => p,
        Err(e) => return json!({"datasets": [], "failed": [{"why": e.to_string()}]}),
    };
    for p in places
        .into_iter()
        .filter(|p| p.role == Role::Source && !place::is_root(&p.dataset))
    {
        let done = (|| -> Result<Option<Value>, StoreError> {
            let sources = crate::sources::source_ids(store, &p)?;
            if sources.is_empty() || !judged(store, &sources, job)? {
                return Ok(None);
            }
            let mut signs = read(store, &sources, Some(pack))?;
            signs["read"] = json!("at the end of a sort");
            signs["job"] = json!(job);
            round::set_headers_signs(store, p.id, &signs)?;
            let person = round::headers_flag(store, p.id)?;
            Ok(Some(json!({
                "dataset": p.name,
                "damaged": person.as_ref().map_or(signs["damaged"].clone(), |f| f["damaged"].clone()),
                "by": if person.is_some() { "person" } else { "signs" },
                "signs": signs["damaged"], "why": signs["why"],
            })))
        })();
        match done {
            Ok(Some(v)) => read_now.push(v),
            Ok(None) => {}
            Err(e) => failed.push(json!({"dataset": p.name, "why": e.to_string()})),
        }
    }
    json!({"datasets": read_now, "failed": failed})
}

/// Whether the sort `job` judged a scan these sources hold.
fn judged(store: &mut Store, sources: &[i64], job: i64) -> Result<bool, StoreError> {
    let holds = crate::operations::held_by(store, "x", &list(sources));
    let sql = format!(
        "SELECT COUNT(*) FROM {} c JOIN {} x ON x.id = c.stack_id WHERE c.job_id = {} AND {holds}",
        store.qualified("classification"),
        store.qualified("stack"),
        store.dialect().param(1, Type::Int)
    );
    Ok(store
        .query_opt(&sql, &[Param::Int(job)])?
        .map(|r| r.int(0))
        .transpose()?
        .unwrap_or(0)
        > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_share_is_said_as_the_card_says_it() {
        assert_eq!(pct(0.0), "0 %");
        assert_eq!(pct(0.02), "2 %");
        assert_eq!(pct(0.0209), "2.1 %");
        assert_eq!(pct(0.1), "10 %");
        assert_eq!(pct(0.2417), "24 %");
    }

    #[test]
    fn a_sign_flags_at_its_threshold_and_only_with_one() {
        assert!(reaches(0.02, Some(0.02), 100));
        assert!(!reaches(0.019, Some(0.02), 100));
        assert!(!reaches(1.0, None, 100), "no threshold flags nothing");
        assert!(!reaches(0.0, Some(0.0), 0), "no scans flag nothing");
    }
}
