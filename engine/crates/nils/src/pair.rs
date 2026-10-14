// SPDX-License-Identifier: AGPL-3.0-only

//! Pair mode at the keyboard and the door (the post-contrast study, P6). The
//! mechanism is the registry's (`nils_registry::pair`); here a campaign is
//! made from a list of pairs, and the doors serve an item's two stacks,
//! how the campaign goes, and each answer resolved per stack.
//!
//! - **Making one.** `nils campaign pair <name> --pairs FILE --seed TEXT`:
//!   one pair a line, two stack ids. The two stacks of a pair are of one
//!   subject; the seed draws which is shown on the left and the order the
//!   pairs are read in, and the campaign keeps the seed's sha256, never the
//!   seed. The campaign suggests nothing.
//! - **Blind.** The sheet is the two stacks and the five answers. Every door
//!   that would read a stack's file, its header, the rules' values or a
//!   suggestion refuses a pair campaign, so neither a time, nor a series
//!   name, nor what the rules said reaches the person reading.
//! - **Resolved.** `GET /api/campaigns/{id}/pair/values` and `nils campaign
//!   pair-export`: one row per answer and stack with the value of the axis
//!   the answer comes to.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use clap::Args;
use nils_registry::campaign::{self, Items, New};
use nils_registry::home::Home;
use nils_registry::pair;
use nils_registry::schema::Type;
use nils_registry::store::Param;
use nils_registry::{Registry, Store};
use serde_json::{Value, json};

use crate::campaigns::{campaign_err, cerr, print, rerr, shown, who};
use crate::drawn::Refused;
use crate::serve::{Caller, Reply};
use crate::{Exit, usage};

#[derive(Debug, Args)]
pub(crate) struct PairArgs {
    /// The campaign's name
    name: String,
    /// The pairs, one a line: two stack ids, by a tab, a comma or spaces;
    /// a line starting with # and a header line are skipped
    #[arg(long, value_name = "FILE")]
    pairs: Option<PathBuf>,
    /// One pair, two stack ids joined by a comma; repeat for more
    #[arg(long = "pair", value_name = "A,B")]
    pair: Vec<String>,
    /// The seed that draws each pair's sides and the order they are read
    /// in, written down before the draw
    #[arg(long, value_name = "TEXT")]
    seed: String,
    /// The axis each answer resolves into
    #[arg(long, default_value = pair::AXIS, value_name = "AXIS")]
    axis: String,
    /// The axis's value for a stack the answer says is post
    #[arg(long, default_value = pair::POST, value_name = "VALUE")]
    post: String,
    /// The axis's value for a stack the answer says is pre
    #[arg(long, default_value = pair::PRE, value_name = "VALUE")]
    pre: String,
    /// Who reads the pairs; anyone with campaigns:work when none is named
    #[arg(long = "rater", value_name = "PRINCIPAL")]
    raters: Vec<String>,
    /// How many people read each pair (a second reader for agreement)
    #[arg(long, default_value_t = 1, value_name = "N")]
    raters_per_item: i64,
    #[arg(long, default_value_t = 3600, value_name = "SECONDS")]
    lease_seconds: i64,
    #[arg(long, value_name = "DIR")]
    pack_dir: Option<PathBuf>,
    #[arg(long, default_value = "mri")]
    pack: String,
    /// Check the pairs and count them, and make nothing
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    json: bool,
}

/// One pair from a line or an argument: two stack ids.
fn pair_of(text: &str) -> Option<(i64, i64)> {
    let ids: Vec<&str> = text
        .split(|c: char| c == ',' || c == '\t' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .collect();
    match ids.as_slice() {
        [a, b] => Some((a.parse().ok()?, b.parse().ok()?)),
        _ => None,
    }
}

/// The pairs a file names: one a line; blank lines, lines starting with #
/// and a first line that is not two ids (a header) are skipped.
fn pairs_in(text: &str) -> Result<Vec<(i64, i64)>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        match pair_of(t) {
            Some(p) => out.push(p),
            None if out.is_empty() && n == 0 => {}
            None => return Err(format!("line {}: two stack ids, not {t:?}", n + 1)),
        }
    }
    Ok(out)
}

/// Each stack's subject and study, where the registry has the stack, for
/// a door or the keyboard alike.
pub(crate) fn whose_of(
    store: &mut Store,
    stacks: &[i64],
) -> Result<BTreeMap<i64, (i64, i64)>, nils_registry::store::Error> {
    let mut out = BTreeMap::new();
    for chunk in stacks.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT k.id, st.subject_id, st.id FROM {} k JOIN {} r ON r.id = k.series_id \
             JOIN {} st ON st.id = r.study_id WHERE k.id IN ({list})",
            store.qualified("stack"),
            store.qualified("series"),
            store.qualified("study"),
        );
        for r in store.query(&sql, &[])? {
            out.insert(r.int(0)?, (r.int(1)?, r.int(2)?));
        }
    }
    Ok(out)
}

/// `nils campaign pair`: check the pairs, draw their sides and order, and
/// make the campaign.
pub(crate) fn make(home: &Home, a: PairArgs) -> Result<(), Exit> {
    let mut named: Vec<(i64, i64)> = Vec::new();
    if let Some(f) = &a.pairs {
        let text =
            std::fs::read_to_string(f).map_err(|e| usage(format!("{}: {e}", f.display())))?;
        named.extend(pairs_in(&text).map_err(|m| usage(format!("{}: {m}", f.display())))?);
    }
    for p in &a.pair {
        named.push(pair_of(p).ok_or_else(|| usage(format!("--pair {p}: two stack ids, A,B")))?);
    }
    if named.is_empty() {
        return Err(usage(
            "a pair campaign needs pairs: --pairs FILE, --pair A,B",
        ));
    }
    let question = json!({"kind": "pair", "axis": a.axis, "post": a.post, "pre": a.pre});
    let served = crate::pack_dir(home, a.pack_dir.clone())
        .ok()
        .and_then(|d| nils_pack::load(&d.join(&a.pack), None).ok());
    let mut registry = crate::open(home)?;
    let principal = who();
    let (out, made) = make_pairs(
        &mut registry,
        &crate::drawn::Making {
            name: &a.name,
            owner: &principal,
            question,
            seed: &a.seed,
            raters: a.raters.clone(),
            raters_per_item: a.raters_per_item,
            lease_seconds: a.lease_seconds,
            served: served.as_ref(),
            dry_run: a.dry_run,
            counts: serde_json::Map::new(),
        },
        &named,
    )
    .map_err(crate::drawn::exit)?;
    match made {
        Some(made) if a.json => {
            let mut v = shown(registry.store(), &made).map_err(rerr)?;
            v["pair"] = out;
            print(&v);
            Ok(())
        }
        _ => report(&out, a.json),
    }
}

/// The axis and its two values of a pair or an anchored question, checked
/// against the served pack where one is served.
pub(crate) fn check_axis(
    question: &Value,
    served: Option<&nils_pack::Pack>,
) -> Result<(), Refused> {
    campaign::Question::parse(question).map_err(|e| (400, e.to_string()))?;
    let Some(p) = served else {
        return Ok(());
    };
    let name = question["axis"].as_str().unwrap_or(pair::AXIS);
    let axis = p
        .axes
        .iter()
        .find(|x| x.name == name)
        .ok_or_else(|| (400, format!("axis {name}: the pack has no such axis")))?;
    for key in ["post", "pre"] {
        if let Some(v) = question[key].as_str()
            && !axis.values.iter().any(|x| x.id == v)
        {
            return Err((400, format!("{key}: {v} is not a value of {name}")));
        }
    }
    Ok(())
}

/// A pair campaign made from the pairs named, at the keyboard or the door:
/// the sides and the order drawn by the seed, the two stacks of a pair one
/// person's, the campaign suggesting nothing, and the make audited. With
/// `dry_run`, the counts alone. Answers what was made, with its counts and
/// its pictures, and the campaign.
pub(crate) fn make_pairs(
    registry: &mut Registry,
    m: &crate::drawn::Making<'_>,
    named: &[(i64, i64)],
) -> Result<(Value, Option<campaign::Campaign>), Refused> {
    if m.seed.trim().is_empty() {
        return Err((
            400,
            "seed: the seed that draws the sides, written down before the draw".into(),
        ));
    }
    if named.is_empty() {
        return Err((400, "a pair campaign needs pairs".into()));
    }
    check_axis(&m.question, m.served)?;
    let drawn = pair::draw(named, m.seed).map_err(|e| (400, e.to_string()))?;
    let stacks: Vec<i64> = drawn
        .iter()
        .flat_map(|&(l, r)| [l, r])
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let of = whose_of(registry.store(), &stacks).map_err(|e| (500, e.to_string()))?;
    if let Some(s) = stacks.iter().find(|s| !of.contains_key(s)) {
        return Err((400, format!("stack {s} is not in the registry")));
    }
    // the two stacks of a pair are one person's; one session's, as the pair
    // step found them, and most often one study's
    let mut across_studies = 0usize;
    for &(l, r) in &drawn {
        let ((sl, tl), (sr, tr)) = (of[&l], of[&r]);
        if sl != sr {
            return Err((
                400,
                format!(
                    "stacks {} and {} are two people's; a pair is of one session",
                    l.min(r),
                    l.max(r)
                ),
            ));
        }
        if tl != tr {
            across_studies += 1;
        }
    }
    let shared = {
        let mut seen: BTreeMap<i64, usize> = BTreeMap::new();
        for &(l, r) in &drawn {
            *seen.entry(l).or_default() += 1;
            *seen.entry(r).or_default() += 1;
        }
        seen.values().filter(|n| **n > 1).count()
    };
    let seed_sha256 = crate::campaigns::sha256(m.seed.as_bytes());
    let mut counts = json!({
        "pairs": drawn.len(), "stacks": stacks.len(),
        "stacks_in_two_or_more_pairs": shared, "pairs_across_studies": across_studies,
    });
    for (k, v) in &m.counts {
        counts[k.as_str()] = v.clone();
    }
    let mut out = json!({
        "name": m.name, "counts": counts, "seed_sha256": seed_sha256, "dry_run": m.dry_run,
    });
    if m.dry_run {
        return Ok((out, None));
    }
    let source = json!({
        "pair": {"seed_sha256": seed_sha256, "pairs": drawn.len()},
    });
    let pack_version = m.served.map(|p| format!("{}@{}", p.name, p.version));
    let made = campaign::create(
        registry,
        &New {
            name: m.name,
            owner: m.owner,
            question: &m.question,
            source,
            items: Items::Pairs(drawn),
            handle_id: None,
            content_hash: None,
            pack_version: pack_version.as_deref(),
            raters_per_item: m.raters_per_item,
            raters: m.raters.clone(),
            adjudicators: Vec::new(),
            adjudication: &json!({"when": "never", "metric": "exact"}),
            closes_into: "none",
            lease_seconds: m.lease_seconds,
            inputs: BTreeMap::new(),
            hold_back: None,
            // the two pictures are all there is: nothing is suggested
            suggest: Some(campaign::Suggest::None),
            hide_header: false,
        },
    )
    .map_err(|e| crate::drawn::refused(campaign_err(e)))?;
    nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal: m.owner,
            action: nils_registry::audit::Action::CampaignPair,
            scope: json!({"campaign": made.id, "name": made.name}),
            policy: None,
            job_id: None,
            details: Some(json!({"counts": out["counts"], "seed_sha256": seed_sha256})),
        },
    )
    .map_err(|e| (500, e.to_string()))?;
    out["campaign"] = json!(made.id);
    out["pictures"] = crate::pyramid::pictures(registry.store(), &stacks);
    Ok((out, Some(made)))
}

fn report(out: &Value, json: bool) -> Result<(), Exit> {
    if json {
        print(out);
        return Ok(());
    }
    let c = &out["counts"];
    let name = out["name"].as_str().unwrap_or_default();
    match out["campaign"].as_i64() {
        Some(id) => println!("campaign {id} {name}: {} pair(s) to read", c["pairs"]),
        None => println!("would make {name}: {} pair(s) to read", c["pairs"]),
    }
    println!(
        "  {} stack(s), {} in two pairs or more, {} pair(s) across two studies",
        c["stacks"], c["stacks_in_two_or_more_pairs"], c["pairs_across_studies"]
    );
    println!(
        "  seed sha256 {}",
        out["seed_sha256"].as_str().unwrap_or_default()
    );
    if let Some(m) = out["pictures"]["missing"].as_u64().filter(|m| *m > 0) {
        println!("  {m} stack(s) have no picture yet: nils pyramid build --stack ID");
    }
    Ok(())
}

/// `nils campaign pair-export`: every answer resolved per stack.
pub(crate) fn export(home: &Home, which: &str, out: Option<&Path>) -> Result<(), Exit> {
    let mut registry = crate::open(home)?;
    let c = campaign::find(registry.store(), which).map_err(cerr)?;
    if !pair::is_pair(&c) {
        return Err(usage(format!("campaign {} asks of no pair", c.name)));
    }
    let rows = pair::values(registry.store(), &c).map_err(cerr)?;
    let summary = pair::summary(registry.store(), &c, &rows).map_err(cerr)?;
    let doc = json!({
        "campaign": c.id, "name": c.name, "status": c.status,
        "question": c.question, "seed_sha256": c.source["pair"]["seed_sha256"],
        "summary": summary, "values": rows,
    });
    match out {
        Some(p) => {
            std::fs::write(p, serde_json::to_string_pretty(&doc).unwrap_or_default())
                .map_err(|e| usage(format!("{}: {e}", p.display())))?;
            println!("wrote {}", p.display());
            Ok(())
        }
        None => {
            print(&doc);
            Ok(())
        }
    }
}

// ------------------------------------------------------------ the doors

fn no_pair(c: &campaign::Campaign) -> Reply {
    Reply::error(409, format!("campaign {} asks of no pair", c.name))
}

/// A pair campaign's doors that would show more than the two pictures:
/// a stack's evidence line, its header, what the pack derives, a session's
/// candidates, an A/B sheet, a batch, the gallery, suggestions and the
/// combinations. Each refuses a pair campaign.
pub(crate) fn shows_more(rest: &[&str]) -> bool {
    matches!(
        rest,
        [
            "items",
            _,
            "why" | "header" | "derive" | "candidates" | "ab"
        ] | ["batches", ..]
            | ["gallery", ..]
            | ["suggestions"]
            | ["combinations"]
            | ["ab", ..]
    )
}

/// The refusal of such a door.
pub(crate) fn refusal(c: &campaign::Campaign) -> Reply {
    Reply::error(
        409,
        format!(
            "campaign {} reads {} from their pictures alone: no time, no name, no header, nothing suggested, one at a time",
            c.name,
            if nils_registry::anchored::is_anchored(c) {
                "a stack beside its anchors"
            } else {
                "pairs"
            }
        ),
    )
}

/// `GET /api/campaigns/{id}/items/{item}/pair`: the two stacks of one pair
/// and the answers, for the caller who holds it (or a holder of
/// review:work).
pub(crate) fn sheet_door(
    registry: &mut Registry,
    caller: &Caller,
    c: &campaign::Campaign,
    item: i64,
) -> Result<Reply, Reply> {
    if !pair::is_pair(c) {
        return Err(no_pair(c));
    }
    let holds = campaign::assignments(registry.store(), c.id)
        .map_err(campaign_err)?
        .iter()
        .any(|a| a.item_id == item && a.principal.as_deref() == Some(caller.principal.as_str()));
    if !holds && !caller.access.holds("review:work") {
        return Err(Reply::error(
            403,
            format!("item {item} is read by who claimed it; claim it first"),
        ));
    }
    let doc = pair::sheet(registry.store(), c, item)
        .map_err(campaign_err)?
        .ok_or_else(|| Reply::error(404, format!("item {item} is no pair")))?;
    Ok(Reply::ok(doc))
}

/// The answers resolved per stack that the caller reads: their own while
/// the campaign is open, every one once it is closed or for an adjudicator
/// or a holder of review:work (as `GET /api/campaigns/{id}/answers`).
fn readable(
    registry: &mut Registry,
    caller: &Caller,
    c: &campaign::Campaign,
) -> Result<(Vec<Value>, bool), Reply> {
    let principal = caller.principal.as_str();
    let all = crate::campaigns::sees_all(registry.store(), caller, principal, c)?;
    let rows: Vec<Value> = pair::values(registry.store(), c)
        .map_err(campaign_err)?
        .into_iter()
        .filter(|r| all || r["principal"].as_str() == Some(principal))
        .collect();
    Ok((rows, !all))
}

/// `GET /api/campaigns/{id}/pair`: how the campaign goes.
pub(crate) fn summary_door(
    registry: &mut Registry,
    caller: &Caller,
    c: &campaign::Campaign,
) -> Result<Reply, Reply> {
    if !pair::is_pair(c) {
        return Err(no_pair(c));
    }
    let (rows, blind) = readable(registry, caller, c)?;
    let mut doc = pair::summary(registry.store(), c, &rows).map_err(campaign_err)?;
    doc["blind"] = json!(blind);
    doc["seed_sha256"] = c.source["pair"]["seed_sha256"].clone();
    Ok(Reply::ok(doc))
}

/// `GET /api/campaigns/{id}/pair/values`: each answer resolved per stack.
pub(crate) fn values_door(
    registry: &mut Registry,
    caller: &Caller,
    c: &campaign::Campaign,
) -> Result<Reply, Reply> {
    if !pair::is_pair(c) {
        return Err(no_pair(c));
    }
    let (rows, blind) = readable(registry, caller, c)?;
    Ok(Reply::ok(json!({
        "campaign": c.id, "name": c.name, "status": c.status,
        "count": rows.len(), "values": rows, "blind": blind,
    })))
}

/// The open pair campaigns that show a stack, for the pictures a rater
/// reads through one.
pub(crate) fn campaigns_showing(store: &mut Store, stack: i64) -> Result<Vec<i64>, Reply> {
    let sql = format!(
        "SELECT DISTINCT s.campaign_id FROM {} s JOIN {} c ON c.id = s.campaign_id \
         WHERE c.status = 'open' AND s.stack_id = {}",
        store.qualified("campaign_pair_side"),
        store.qualified("campaign"),
        store.dialect().param(1, Type::Int)
    );
    let mut out = Vec::new();
    for r in store.query(&sql, &[Param::Int(stack)])? {
        out.push(r.int(0)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_are_read_from_lines_with_any_separator() {
        let got = pairs_in("left\tright\n12\t14\n# a rerun\n15,16\n 17 18 \n\n").unwrap();
        assert_eq!(got, vec![(12, 14), (15, 16), (17, 18)]);
        assert!(pairs_in("12\t14\n15\n").is_err());
        assert_eq!(pair_of("3,4"), Some((3, 4)));
        assert_eq!(pair_of("3"), None);
    }

    #[test]
    fn the_doors_that_show_more_than_the_pictures_are_named() {
        for rest in [
            &["items", "4", "why"][..],
            &["items", "4", "header"],
            &["items", "4", "derive"],
            &["batches"],
            &["gallery", "accept"],
            &["suggestions"],
            &["combinations"],
            &["ab", "decisions"],
        ] {
            assert!(shows_more(rest), "{rest:?}");
        }
        for rest in [&["items", "4", "pair"][..], &["claim"], &["pair", "values"]] {
            assert!(!shows_more(rest), "{rest:?}");
        }
    }
}
