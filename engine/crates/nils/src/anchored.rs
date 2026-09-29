// SPDX-License-Identifier: AGPL-3.0-only

//! Anchored reading at the keyboard and the door (the post-contrast study,
//! record 48 of 2026-09-29, night). The mechanism is the registry's
//! (`nils_registry::anchored`); here a campaign is made from a list of
//! items, and the doors serve an item's three panels, how the campaign
//! goes, and each answer resolved for its candidate.
//!
//! - **Making one.** `nils campaign anchored <name> --items FILE --seed
//!   TEXT`: one item a line, a candidate, its pre anchor and its post
//!   anchor (tab, comma or spaces; a header naming `candidate`,
//!   `pre_anchor` and `post_anchor` may put them in any order). The three
//!   are one subject's; an anchor of another session than its candidate's
//!   is allowed and flagged. The seed draws each item's panels and the
//!   order the items are read in; the campaign keeps the seed's sha256,
//!   never the seed, and suggests nothing.
//! - **Blind.** The sheet is the three panels, labelled candidate,
//!   reference pre and reference post, and the three answers. Every door
//!   that would read a stack's file, its header, the rules' values or a
//!   suggestion refuses an anchored campaign, as it does a pair campaign.
//! - **Resolved.** `GET /api/campaigns/{id}/anchored/values` and `nils
//!   campaign anchored-export`: one row per answer with the candidate's
//!   value; the anchors are named and never given one.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use clap::Args;
use nils_registry::Registry;
use nils_registry::anchored::{self, Named};
use nils_registry::campaign::{self, Items, New};
use nils_registry::home::Home;
use nils_registry::schema::Type;
use nils_registry::store::{Param, Store};
use serde_json::{Value, json};

use crate::campaigns::{campaign_err, cerr, print, rerr, shown, who};
use crate::serve::{Caller, Reply};
use crate::{Exit, usage};

#[derive(Debug, Args)]
pub(crate) struct AnchoredArgs {
    /// The campaign's name
    name: String,
    /// The items, one a line: the candidate, its pre anchor and its post
    /// anchor, by a tab, a comma or spaces. A header line naming
    /// candidate, pre_anchor and post_anchor puts them in its order (other
    /// columns are skipped); without one the first three columns are taken.
    /// Lines starting with # are skipped
    #[arg(long, value_name = "FILE")]
    items: Option<PathBuf>,
    /// One item, three stack ids: candidate,pre,post; repeat for more
    #[arg(long = "item", value_name = "C,PRE,POST")]
    item: Vec<String>,
    /// The seed that draws each item's panels and the order they are read
    /// in, written down before the draw
    #[arg(long, value_name = "TEXT")]
    seed: String,
    /// The axis the answer resolves into for the candidate
    #[arg(long, default_value = nils_registry::pair::AXIS, value_name = "AXIS")]
    axis: String,
    /// The axis's value for a candidate read as like the post
    #[arg(long, default_value = nils_registry::pair::POST, value_name = "VALUE")]
    post: String,
    /// The axis's value for a candidate read as like the pre
    #[arg(long, default_value = nils_registry::pair::PRE, value_name = "VALUE")]
    pre: String,
    /// Who reads the items; anyone with campaigns:work when none is named
    #[arg(long = "rater", value_name = "PRINCIPAL")]
    raters: Vec<String>,
    /// How many people read each item
    #[arg(long, default_value_t = 1, value_name = "N")]
    raters_per_item: i64,
    #[arg(long, default_value_t = 3600, value_name = "SECONDS")]
    lease_seconds: i64,
    #[arg(long, value_name = "DIR")]
    pack_dir: Option<PathBuf>,
    #[arg(long, default_value = "mri")]
    pack: String,
    /// Check the items and count them, and make nothing
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    json: bool,
}

fn fields(text: &str) -> Vec<&str> {
    text.split(|c: char| c == ',' || c == '\t' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .collect()
}

/// One item from an argument: three stack ids, candidate, pre, post.
fn item_of(text: &str) -> Option<Named> {
    match fields(text).as_slice() {
        [c, p, q] => Some(Named {
            candidate: c.parse().ok()?,
            pre: p.parse().ok()?,
            post: q.parse().ok()?,
        }),
        _ => None,
    }
}

/// The columns a header line names: candidate, pre anchor, post anchor.
fn header_of(line: &str) -> Option<[usize; 3]> {
    let cols: Vec<String> = fields(line).iter().map(|s| s.to_lowercase()).collect();
    let at = |names: &[&str]| cols.iter().position(|c| names.contains(&c.as_str()));
    Some([
        at(&["candidate", "candidate_stack", "stack"])?,
        at(&["pre_anchor", "pre", "anchor_pre", "reference_pre"])?,
        at(&["post_anchor", "post", "anchor_post", "reference_post"])?,
    ])
}

/// The items a file names, one a line. A first line that is not ids is a
/// header: it must name the three columns, and then puts them in its order.
fn items_in(text: &str) -> Result<Vec<Named>, String> {
    let mut out = Vec::new();
    let mut cols: Option<[usize; 3]> = None;
    let mut first = true;
    for (n, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let f = fields(t);
        let numeric = f.iter().all(|x| x.parse::<i64>().is_ok());
        if first && !numeric {
            first = false;
            cols = Some(header_of(t).ok_or_else(|| {
                format!(
                    "line {}: a header names candidate, pre_anchor and post_anchor, not {t:?}",
                    n + 1
                )
            })?);
            continue;
        }
        first = false;
        let [c, p, q] = cols.unwrap_or([0, 1, 2]);
        let id = |i: usize| f.get(i).and_then(|x| x.parse::<i64>().ok());
        match (id(c), id(p), id(q)) {
            (Some(candidate), Some(pre), Some(post)) => out.push(Named {
                candidate,
                pre,
                post,
            }),
            _ => {
                return Err(format!(
                    "line {}: a candidate, a pre anchor and a post anchor, not {t:?}",
                    n + 1
                ));
            }
        }
    }
    Ok(out)
}

/// `nils campaign anchored`: check the items, draw their panels and order,
/// and make the campaign.
pub(crate) fn make(home: &Home, a: AnchoredArgs) -> Result<(), Exit> {
    if a.seed.trim().is_empty() {
        return Err(usage(
            "--seed: the seed that draws the panels, written down before the draw",
        ));
    }
    let mut named: Vec<Named> = Vec::new();
    if let Some(f) = &a.items {
        let text =
            std::fs::read_to_string(f).map_err(|e| usage(format!("{}: {e}", f.display())))?;
        named.extend(items_in(&text).map_err(|m| usage(format!("{}: {m}", f.display())))?);
    }
    for i in &a.item {
        named
            .push(item_of(i).ok_or_else(|| {
                usage(format!("--item {i}: three stack ids, candidate,pre,post"))
            })?);
    }
    if named.is_empty() {
        return Err(usage(
            "an anchored campaign needs items: --items FILE, --item C,PRE,POST",
        ));
    }
    let question = json!({"kind": "anchored", "axis": a.axis, "post": a.post, "pre": a.pre});
    campaign::Question::parse(&question).map_err(cerr)?;
    let served = crate::pack_dir(home, a.pack_dir.clone())
        .ok()
        .and_then(|d| nils_pack::load(&d.join(&a.pack), None).ok());
    if let Some(p) = &served {
        let axis = p
            .axes
            .iter()
            .find(|x| x.name == a.axis)
            .ok_or_else(|| usage(format!("--axis {}: the pack has no such axis", a.axis)))?;
        for v in [&a.post, &a.pre] {
            if !axis.values.iter().any(|x| &x.id == v) {
                return Err(usage(format!("{v} is not a value of {}", a.axis)));
            }
        }
    }
    let mut registry = crate::open(home)?;
    let stacks: Vec<i64> = named
        .iter()
        .flat_map(|n| [n.candidate, n.pre, n.post])
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let of = crate::pair::whose(registry.store(), &stacks)?;
    if let Some(s) = stacks.iter().find(|s| !of.contains_key(s)) {
        return Err(usage(format!("stack {s} is not in the registry")));
    }
    // the three stacks of an item are one person's; an anchor from another
    // session (study) than the candidate's is allowed, and flagged
    let mut other: BTreeMap<i64, (bool, bool)> = BTreeMap::new();
    for n in &named {
        let (sc, tc) = of[&n.candidate];
        for anchor in [n.pre, n.post] {
            if of[&anchor].0 != sc {
                return Err(usage(format!(
                    "stacks {} and {anchor} are two people's; an anchor is of the candidate's subject",
                    n.candidate
                )));
            }
        }
        other.insert(n.candidate, (of[&n.pre].1 != tc, of[&n.post].1 != tc));
    }
    let drawn = anchored::draw(&named, &other, &a.seed).map_err(cerr)?;
    let candidates: BTreeSet<i64> = named.iter().map(|n| n.candidate).collect();
    let anchors: BTreeSet<i64> = named.iter().flat_map(|n| [n.pre, n.post]).collect();
    let pre_other = drawn.iter().filter(|s| s.pre_other_session).count();
    let post_other = drawn.iter().filter(|s| s.post_other_session).count();
    let any_other = drawn
        .iter()
        .filter(|s| s.pre_other_session || s.post_other_session)
        .count();
    // a stack that is a candidate here and an anchor there would show the
    // reader its label: counted, so the list can be mended before it is made
    let both = candidates.intersection(&anchors).count();
    let seed_sha256 = crate::campaigns::sha256(a.seed.as_bytes());
    let counts = json!({
        "items": drawn.len(), "stacks": stacks.len(), "anchors": anchors.len(),
        "items_with_an_anchor_of_another_session": any_other,
        "pre_anchors_of_another_session": pre_other,
        "post_anchors_of_another_session": post_other,
        "candidates_that_are_also_anchors": both,
    });
    let mut out = json!({
        "name": a.name, "counts": counts, "seed_sha256": seed_sha256, "dry_run": a.dry_run,
    });
    if a.dry_run {
        return report(&out, a.json);
    }
    let source = json!({
        "anchored": {"seed_sha256": seed_sha256, "items": drawn.len()},
    });
    let pack_version = served.as_ref().map(|p| format!("{}@{}", p.name, p.version));
    let principal = who();
    let made = campaign::create(
        &mut registry,
        &New {
            name: &a.name,
            owner: &principal,
            question: &question,
            source,
            items: Items::Anchored(drawn),
            handle_id: None,
            content_hash: None,
            pack_version: pack_version.as_deref(),
            raters_per_item: a.raters_per_item,
            raters: a.raters.clone(),
            adjudicators: Vec::new(),
            adjudication: &json!({"when": "never", "metric": "exact"}),
            closes_into: "none",
            lease_seconds: a.lease_seconds,
            inputs: BTreeMap::new(),
            hold_back: None,
            // the three pictures are all there is: nothing is suggested
            suggest: Some(campaign::Suggest::None),
            // the header doors are refused for an anchored campaign as a whole
            hide_header: false,
        },
    )
    .map_err(cerr)?;
    nils_registry::audit::record(
        &mut registry,
        &nils_registry::audit::Entry {
            principal: &principal,
            action: nils_registry::audit::Action::CampaignAnchored,
            scope: json!({"campaign": made.id, "name": made.name}),
            policy: None,
            job_id: None,
            details: Some(json!({"counts": counts, "seed_sha256": seed_sha256})),
        },
    )
    .map_err(|e| crate::fail(e.to_string()))?;
    let pictures = crate::pyramid::pictures(registry.store(), &stacks);
    out["campaign"] = json!(made.id);
    out["pictures"] = pictures;
    if a.json {
        let mut v = shown(registry.store(), &made).map_err(rerr)?;
        v["anchored"] = out;
        print(&v);
        return Ok(());
    }
    report(&out, false)
}

fn report(out: &Value, json: bool) -> Result<(), Exit> {
    if json {
        print(out);
        return Ok(());
    }
    let c = &out["counts"];
    let name = out["name"].as_str().unwrap_or_default();
    match out["campaign"].as_i64() {
        Some(id) => println!("campaign {id} {name}: {} item(s) to read", c["items"]),
        None => println!("would make {name}: {} item(s) to read", c["items"]),
    }
    println!(
        "  {} stack(s), {} anchor(s); {} item(s) with an anchor of another session ({} pre, {} post)",
        c["stacks"],
        c["anchors"],
        c["items_with_an_anchor_of_another_session"],
        c["pre_anchors_of_another_session"],
        c["post_anchors_of_another_session"]
    );
    if c["candidates_that_are_also_anchors"].as_u64().unwrap_or(0) > 0 {
        println!(
            "  warning: {} candidate(s) are an anchor of another item, where the reader sees their label",
            c["candidates_that_are_also_anchors"]
        );
    }
    println!(
        "  seed sha256 {}",
        out["seed_sha256"].as_str().unwrap_or_default()
    );
    if let Some(m) = out["pictures"]["missing"].as_u64().filter(|m| *m > 0) {
        println!("  {m} stack(s) have no picture yet: nils pyramid build --stack ID");
    }
    Ok(())
}

/// `nils campaign anchored-export`: every answer resolved for its candidate.
pub(crate) fn export(home: &Home, which: &str, out: Option<&Path>) -> Result<(), Exit> {
    let mut registry = crate::open(home)?;
    let c = campaign::find(registry.store(), which).map_err(cerr)?;
    if !anchored::is_anchored(&c) {
        return Err(usage(format!(
            "campaign {} asks of no anchored item",
            c.name
        )));
    }
    let rows = anchored::values(registry.store(), &c).map_err(cerr)?;
    let summary = anchored::summary(registry.store(), &c, &rows).map_err(cerr)?;
    let doc = json!({
        "campaign": c.id, "name": c.name, "status": c.status,
        "question": c.question, "seed_sha256": c.source["anchored"]["seed_sha256"],
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

fn not_anchored(c: &campaign::Campaign) -> Reply {
    Reply::error(409, format!("campaign {} asks of no anchored item", c.name))
}

/// `GET /api/campaigns/{id}/items/{item}/anchored`: the three panels of one
/// item and the answers, for the caller who holds it (or a holder of
/// review:work).
pub(crate) fn sheet_door(
    registry: &mut Registry,
    caller: &Caller,
    c: &campaign::Campaign,
    item: i64,
) -> Result<Reply, Reply> {
    if !anchored::is_anchored(c) {
        return Err(not_anchored(c));
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
    let doc = anchored::sheet(registry.store(), c, item)
        .map_err(campaign_err)?
        .ok_or_else(|| Reply::error(404, format!("item {item} is no anchored item")))?;
    Ok(Reply::ok(doc))
}

fn readable(
    registry: &mut Registry,
    caller: &Caller,
    c: &campaign::Campaign,
) -> Result<(Vec<Value>, bool), Reply> {
    let principal = caller.principal.as_str();
    let all = crate::campaigns::sees_all(registry.store(), caller, principal, c)?;
    let rows: Vec<Value> = anchored::values(registry.store(), c)
        .map_err(campaign_err)?
        .into_iter()
        .filter(|r| all || r["principal"].as_str() == Some(principal))
        .collect();
    Ok((rows, !all))
}

/// `GET /api/campaigns/{id}/anchored`: how the campaign goes.
pub(crate) fn summary_door(
    registry: &mut Registry,
    caller: &Caller,
    c: &campaign::Campaign,
) -> Result<Reply, Reply> {
    if !anchored::is_anchored(c) {
        return Err(not_anchored(c));
    }
    let (rows, blind) = readable(registry, caller, c)?;
    let mut doc = anchored::summary(registry.store(), c, &rows).map_err(campaign_err)?;
    doc["blind"] = json!(blind);
    doc["seed_sha256"] = c.source["anchored"]["seed_sha256"].clone();
    Ok(Reply::ok(doc))
}

/// `GET /api/campaigns/{id}/anchored/values`: each answer resolved for its
/// candidate.
pub(crate) fn values_door(
    registry: &mut Registry,
    caller: &Caller,
    c: &campaign::Campaign,
) -> Result<Reply, Reply> {
    if !anchored::is_anchored(c) {
        return Err(not_anchored(c));
    }
    let (rows, blind) = readable(registry, caller, c)?;
    Ok(Reply::ok(json!({
        "campaign": c.id, "name": c.name, "status": c.status,
        "count": rows.len(), "values": rows, "blind": blind,
    })))
}

/// The open anchored campaigns that show a stack, for the pictures a rater
/// reads through one.
pub(crate) fn campaigns_showing(store: &mut Store, stack: i64) -> Result<Vec<i64>, Reply> {
    let sql = format!(
        "SELECT DISTINCT s.campaign_id FROM {} s JOIN {} c ON c.id = s.campaign_id \
         WHERE c.status = 'open' AND s.stack_id = {}",
        store.qualified("campaign_anchor_panel"),
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
    fn items_are_read_from_lines_with_or_without_a_header() {
        let n = |candidate, pre, post| Named {
            candidate,
            pre,
            post,
        };
        // no header: the first three columns
        let got = items_in("12\t10\t14\n# a rerun\n15,16,17\n 18 19 20 note\n\n").unwrap();
        assert_eq!(got, vec![n(12, 10, 14), n(15, 16, 17), n(18, 19, 20)]);
        // a header puts them in its order, and other columns are skipped
        let got = items_in("subject\tpost_anchor\tcandidate\tpre_anchor\n7\t14\t12\t10\n").unwrap();
        assert_eq!(got, vec![n(12, 10, 14)]);
        assert!(items_in("candidate\tpre\n1\t2\n").is_err());
        assert!(items_in("1\t2\t3\n4\t5\n").is_err());
        assert_eq!(item_of("3,4,5"), Some(n(3, 4, 5)));
        assert_eq!(item_of("3,4"), None);
    }
}
