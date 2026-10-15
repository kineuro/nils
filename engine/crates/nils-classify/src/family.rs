// SPDX-License-Identifier: AGPL-3.0-only

//! The acquisition family the pack's routes recognised in a stack
//! (2026-10-14): SyMRI's, an SWI's, a mix sequence's or STAGE's (record 55,
//! ruling 23). One reading of it, [`acquisition_family`], for every place
//! that says it: a release's names and sidecars, the scans doors, a visit's
//! kinds line and an effect report. It takes the classification as it
//! stands: what the routes' rules said of the stack, the decisions in force
//! over it, and its provenance.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use nils_pack::Pack;
use nils_registry::review::{rank, value_set};
use nils_registry::store::{Error as StoreError, Store};

/// The pack's routes whose acquisitions are grouped as a family, by the
/// name of the rule set, and the family each makes: SyMRI's, SWI's, the mix
/// sequences' (EPIMix and NeuroMix, one route) and STAGE's.
pub const ROUTE_FAMILIES: [(&str, &str); 4] = [
    ("symri", "symri"),
    ("swi", "swi"),
    ("epimix", "mix"),
    ("stage", "stage"),
];

/// The provenance each route enters on, which its products keep and which
/// a name spells as `rec-`.
pub const ENTERED_ON: [(&str, &str); 5] = [
    ("SyMRI", "symri"),
    ("EPIMix", "mix"),
    ("NeuroMix", "mix"),
    ("STAGE", "stage"),
    ("SWIRecon", "swi"),
];

/// What one family's route said of a stack: the rule set, and per axis the
/// values its rules wrote, as rows store them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Said {
    pub route: String,
    pub axes: BTreeMap<String, Vec<String>>,
}

/// The decisions in force on one stack: per axis, the value decided as rows
/// store it, none where it was decided to nothing.
pub type InForce = BTreeMap<String, Option<String>>;

/// A value of an axis as its identity, by the served pack; as written with
/// none, or where the pack does not know it.
fn identity(pack: Option<&Pack>, axis: &str, text: &str) -> String {
    pack.and_then(|p| p.axes.iter().find(|a| a.name == axis))
        .and_then(|a| a.value_named(text).map(|i| a.values[i].id.clone()))
        .unwrap_or_else(|| text.to_string())
}

fn identities<'a>(
    pack: Option<&Pack>,
    axis: &str,
    texts: impl IntoIterator<Item = &'a String>,
) -> BTreeSet<String> {
    texts
        .into_iter()
        .flat_map(|t| value_set(Some(t)))
        .map(|t| identity(pack, axis, &t))
        .collect()
}

/// Whether a decision in force contradicts what a route said: compared as
/// identities, so `T2*w` and `T2starw`, or `FLASH` and `SPGRE`, are one
/// value. On a single-valued axis any other answer does; on a multi-valued
/// one only a decision that drops a value the route wrote, never one that
/// adds others beside it (with no pack served, an axis whose route wrote
/// one value or more is read as multi-valued).
fn contradicts(pack: Option<&Pack>, said: &Said, decided: &InForce) -> bool {
    said.axes.iter().any(|(axis, values)| {
        let Some(d) = decided.get(axis) else {
            return false;
        };
        let route = identities(pack, axis, values);
        let person: BTreeSet<String> = value_set(d.as_deref())
            .into_iter()
            .map(|t| identity(pack, axis, &t))
            .collect();
        let multi = match pack.and_then(|p| p.axes.iter().find(|a| a.name == *axis)) {
            Some(a) => a.multi,
            None => !route.is_empty(),
        };
        match multi {
            true => !route.is_subset(&person),
            false => route != person,
        }
    })
}

/// The acquisition family of a stack, as the classification stands: the
/// family of the route that entered it and decided one of its axes (`said`,
/// in the order its evidence was written; a route enters on its own
/// provenance, so a stack enters one family's route, and the evidence, not
/// a fixed list, settles the rare stack two cite), so an acquisition's own
/// images are in it (SyMRI's acquisition, an SWI's magnitude and phase,
/// which the route calls RawRecon, and its minIP, which it calls
/// ProjectionDerived). A route a decision in force contradicts on an axis
/// it decided ([`contradicts`]) is no longer the stack's: a person
/// corrected the scan against it. Else the family of the route its
/// provenance enters, the decision in force on the provenance over the
/// rules' (a route that entered and had nothing left to decide, as on a
/// display composite, is cited by no evidence); else none.
pub fn acquisition_family(
    pack: Option<&Pack>,
    said: &[Said],
    decided: &InForce,
    provenance: Option<&str>,
) -> Option<&'static str> {
    if let Some(family) = said
        .iter()
        .filter(|s| !contradicts(pack, s, decided))
        .find_map(|s| {
            ROUTE_FAMILIES
                .iter()
                .find(|(set, _)| *set == s.route)
                .map(|(_, family)| *family)
        })
    {
        return Some(family);
    }
    let provenance = match decided.get("provenance") {
        Some(d) => d.as_deref(),
        None => provenance,
    }?;
    let provenance = identity(pack, "provenance", provenance);
    ENTERED_ON
        .iter()
        .find(|(p, _)| *p == provenance)
        .map(|(_, family)| *family)
}

/// What the family routes said of each of `stacks` (every stack where
/// none), as their rules' evidence cites them, in the order it was written.
pub fn said_of(
    store: &mut Store,
    stacks: Option<&[i64]>,
) -> Result<HashMap<i64, Vec<Said>>, StoreError> {
    let named = ROUTE_FAMILIES
        .iter()
        .map(|(set, _)| format!("'{set}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let table = store.qualified("classification_evidence");
    let mut out: HashMap<i64, Vec<Said>> = HashMap::new();
    let mut read = |store: &mut Store, within: String| -> Result<(), StoreError> {
        for r in store.query(
            &format!(
                "SELECT stack_id, rule_set, axis, value FROM {table} \
                 WHERE rule_set IN ({named}) AND author IS NULL{within} ORDER BY id"
            ),
            &[],
        )? {
            let (stack, route, axis) = (r.int(0)?, r.text(1)?, r.text(2)?.to_string());
            let value = r.opt_text(3)?.unwrap_or("").to_string();
            let routes = out.entry(stack).or_default();
            let at = match routes.iter().position(|s| s.route == route) {
                Some(i) => i,
                None => {
                    routes.push(Said {
                        route: route.to_string(),
                        axes: BTreeMap::new(),
                    });
                    routes.len() - 1
                }
            };
            let values = routes[at].axes.entry(axis).or_default();
            if !value.is_empty() && !values.contains(&value) {
                values.push(value);
            }
        }
        Ok(())
    };
    match stacks {
        None => read(store, String::new())?,
        Some(stacks) => {
            for chunk in stacks.chunks(500) {
                if !chunk.is_empty() {
                    read(store, format!(" AND stack_id IN ({})", ids(chunk)))?;
                }
            }
        }
    }
    Ok(out)
}

fn ids(chunk: &[i64]) -> String {
    chunk
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn refs(chunk: &[i64]) -> String {
    chunk
        .iter()
        .map(|i| format!("'{i}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The scopes an origin decision names a stack by, and the fingerprint
/// column each reads, in the order the classifier tries them.
const ORIGIN: [(&str, &str); 3] = [
    ("manufacturer", "manufacturer"),
    ("model", "manufacturer_model_name"),
    ("station", "station_name"),
];

/// The decisions in force on each of `stacks` (every stack where none), on
/// every axis, read once: not withdrawn, committed or never staged, and of
/// those that reach a stack (its own, a review group's it is in, its
/// series', its subject's, its scanner's) the one the classifier applies:
/// the highest rank (a person over an agent over a model), then the
/// narrowest scope, then the latest.
pub fn in_force(
    store: &mut Store,
    stacks: Option<&[i64]>,
) -> Result<HashMap<i64, InForce>, StoreError> {
    // which series and subject each stack is of, and which groups hold it
    let mut tree: HashMap<i64, (i64, i64)> = HashMap::new();
    let mut groups: HashMap<i64, Vec<i64>> = HashMap::new();
    let (stack_t, series_t, member_t) = (
        store.qualified("stack"),
        store.qualified("series"),
        store.qualified("review_member"),
    );
    let tree_sql = |within: &str| {
        format!(
            "SELECT k.id, k.series_id, r.subject_id FROM {stack_t} k JOIN {series_t} r ON r.id = k.series_id{within}"
        )
    };
    let members_sql = |within: &str| format!("SELECT item_id, stack_id FROM {member_t}{within}");
    let chunks: Vec<Option<Vec<i64>>> = match stacks {
        None => vec![None],
        Some(s) => s.chunks(500).map(|c| Some(c.to_vec())).collect(),
    };
    for chunk in &chunks {
        let (t, m) = match chunk {
            None => (tree_sql(""), members_sql("")),
            Some(c) if c.is_empty() => continue,
            Some(c) => (
                tree_sql(&format!(" WHERE k.id IN ({})", ids(c))),
                members_sql(&format!(" WHERE stack_id IN ({})", ids(c))),
            ),
        };
        for r in store.query(&t, &[])? {
            tree.insert(r.int(0)?, (r.int(1)?, r.int(2)?));
        }
        for r in store.query(&m, &[])? {
            groups.entry(r.int(0)?).or_default().push(r.int(1)?);
        }
    }
    // the decisions that may reach them
    struct Row {
        id: i64,
        scope: String,
        reference: String,
        axis: String,
        value: Option<String>,
        rank: u8,
    }
    let base = format!(
        "SELECT id, scope, ref, axis, value, author_kind FROM {} \
         WHERE withdrawn_at IS NULL AND (staged_at IS NULL OR committed_at IS NOT NULL)",
        store.qualified("decision")
    );
    let mut rows: Vec<Row> = Vec::new();
    let mut take = |store: &mut Store, sql: String| -> Result<(), StoreError> {
        for r in store.query(&sql, &[])? {
            rows.push(Row {
                id: r.int(0)?,
                scope: r.text(1)?.to_string(),
                reference: r.text(2)?.to_string(),
                axis: r.text(3)?.to_string(),
                value: r.opt_text(4)?.map(str::to_string),
                rank: rank(r.opt_text(5)?.unwrap_or("person")),
            });
        }
        Ok(())
    };
    match stacks {
        None => take(store, base.clone())?,
        Some(stacks) => {
            let series: Vec<i64> = tree
                .values()
                .map(|(s, _)| *s)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let subjects: Vec<i64> = tree
                .values()
                .map(|(_, s)| *s)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let items: Vec<i64> = groups.keys().copied().collect();
            for (scope, list) in [
                ("stack", stacks.to_vec()),
                ("group", items),
                ("series", series),
                ("subject", subjects),
            ] {
                for chunk in list.chunks(500) {
                    if !chunk.is_empty() {
                        take(
                            store,
                            format!("{base} AND scope = '{scope}' AND ref IN ({})", refs(chunk)),
                        )?;
                    }
                }
            }
            take(store, format!("{base} AND scope = 'origin'"))?;
        }
    }
    // a stack's scanner, where an origin decision may name it
    let mut origin: HashMap<i64, Vec<String>> = HashMap::new();
    if rows.iter().any(|r| r.scope == "origin") {
        let columns = ORIGIN
            .iter()
            .map(|(_, c)| *c)
            .collect::<Vec<_>>()
            .join(", ");
        let fingerprint = store.qualified("stack_fingerprint");
        let sql = |within: &str| format!("SELECT stack_id, {columns} FROM {fingerprint}{within}");
        for chunk in &chunks {
            let q = match chunk {
                None => sql(""),
                Some(c) if c.is_empty() => continue,
                Some(c) => sql(&format!(" WHERE stack_id IN ({})", ids(c))),
            };
            for r in store.query(&q, &[])? {
                let mut texts = Vec::new();
                for i in 0..ORIGIN.len() {
                    texts.push(r.opt_text(i + 1)?.unwrap_or("").trim().to_lowercase());
                }
                origin.insert(r.int(0)?, texts);
            }
        }
    }
    // each row to the stacks it reaches, with its narrowness
    let mut by_stack: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut by_series: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut by_subject: HashMap<i64, Vec<i64>> = HashMap::new();
    for (stack, (series, subject)) in &tree {
        by_stack.entry(*stack).or_default().push(*stack);
        by_series.entry(*series).or_default().push(*stack);
        by_subject.entry(*subject).or_default().push(*stack);
    }
    let wanted = |s: &i64| tree.contains_key(s);
    // (rank, narrowness, id) of the decision holding each (stack, axis)
    // (rank, narrowness, id) and the value, per (stack, axis)
    type Held = ((u8, u8, i64), Option<String>);
    let mut best: HashMap<(i64, String), Held> = HashMap::new();
    let mut offer = |stack: i64, row: &Row, narrowness: u8| {
        let key = (row.rank, narrowness, row.id);
        let slot = best.entry((stack, row.axis.clone()));
        match slot {
            std::collections::hash_map::Entry::Occupied(mut o) => {
                if key > o.get().0 {
                    o.insert((key, row.value.clone()));
                }
            }
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert((key, row.value.clone()));
            }
        }
    };
    for row in &rows {
        let reference = row.reference.parse::<i64>().ok();
        let reached: Vec<i64> = match (row.scope.as_str(), reference) {
            ("stack", Some(id)) => by_stack.get(&id).cloned().unwrap_or_default(),
            ("group", Some(id)) => groups.get(&id).cloned().unwrap_or_default(),
            ("series", Some(id)) => by_series.get(&id).cloned().unwrap_or_default(),
            ("subject", Some(id)) => by_subject.get(&id).cloned().unwrap_or_default(),
            _ => Vec::new(),
        };
        let narrowness = match row.scope.as_str() {
            "stack" => 4,
            "group" => 3,
            "series" => 2,
            "subject" => 1,
            _ => 0,
        };
        for stack in reached.into_iter().filter(|s| wanted(s)) {
            offer(stack, row, narrowness);
        }
    }
    // an origin decision: the first of the scanner's names that one names,
    // on this axis, as the classifier reads them
    let origins: Vec<&Row> = rows.iter().filter(|r| r.scope == "origin").collect();
    if !origins.is_empty() {
        let mut latest: HashMap<(String, String), &Row> = HashMap::new();
        for row in &origins {
            let key = (row.reference.to_lowercase(), row.axis.clone());
            match latest.get(&key) {
                Some(held) if held.rank > row.rank => {}
                _ => {
                    latest.insert(key, row);
                }
            }
        }
        let axes: BTreeSet<String> = origins.iter().map(|r| r.axis.clone()).collect();
        for (stack, texts) in &origin {
            if !wanted(stack) {
                continue;
            }
            for axis in &axes {
                let found = ORIGIN.iter().zip(texts).find_map(|((name, _), text)| {
                    (!text.is_empty())
                        .then(|| latest.get(&(format!("{name}={text}"), axis.clone())))
                        .flatten()
                });
                if let Some(row) = found {
                    offer(*stack, row, 0);
                }
            }
        }
    }
    let mut out: HashMap<i64, InForce> = HashMap::new();
    for ((stack, axis), (_, value)) in best {
        out.entry(stack).or_default().insert(axis, value);
    }
    Ok(out)
}

/// Each of `stacks`' acquisition family (every stack's where none), as the
/// classification stands ([`acquisition_family`]): the routes' evidence and
/// the decisions in force read once, the provenance the rules gave each by
/// `provenance`. A stack in no family is left out.
pub fn families_of(
    store: &mut Store,
    pack: Option<&Pack>,
    stacks: Option<&[i64]>,
    provenance: &dyn Fn(i64) -> Option<String>,
) -> Result<HashMap<i64, &'static str>, StoreError> {
    let decided = in_force(store, stacks)?;
    families_with(store, pack, stacks, &decided, provenance)
}

/// [`families_of`], with the decisions in force already read.
pub fn families_with(
    store: &mut Store,
    pack: Option<&Pack>,
    stacks: Option<&[i64]>,
    decided: &HashMap<i64, InForce>,
    provenance: &dyn Fn(i64) -> Option<String>,
) -> Result<HashMap<i64, &'static str>, StoreError> {
    let said = said_of(store, stacks)?;
    let none = InForce::new();
    let mut out = HashMap::new();
    let mut every: BTreeSet<i64> = said.keys().copied().collect();
    match stacks {
        Some(s) => every.extend(s.iter().copied()),
        None => every.extend(decided.keys().copied()),
    }
    for stack in every {
        let family = acquisition_family(
            pack,
            said.get(&stack).map_or(&[][..], Vec::as_slice),
            decided.get(&stack).unwrap_or(&none),
            provenance(stack).as_deref(),
        );
        if let Some(f) = family {
            out.insert(stack, f);
        }
    }
    Ok(out)
}

/// The word a descriptive name spells a family with.
pub fn word(family: &str) -> &'static str {
    match family {
        "symri" => "SyMRI",
        "swi" => "SWI",
        "mix" => "Mix",
        "stage" => "STAGE",
        _ => "",
    }
}

/// The family a stack's names spell (record 55, ruling 23): its acquisition
/// family, unless the name says it already: a product of the family's route,
/// whose provenance `rec-` spells (`rec-SyMRI`, `rec-SWIRecon`, `rec-STAGE`,
/// `rec-EPIMix`); a stack in a folder of its own (`anat/SyMRI/`); or one a
/// word of whose name is the family's (an SWI's minIP, base SWI; its
/// processed image, construct SWI).
pub fn spelled<'a>(
    family: Option<&'a str>,
    provenance: Option<&str>,
    own_folder: Option<&str>,
    words: &[&str],
) -> Option<&'a str> {
    let family = family?;
    if own_folder.is_some()
        || ENTERED_ON
            .iter()
            .any(|(p, f)| *f == family && provenance == Some(*p))
    {
        return None;
    }
    let word = word(family);
    if words
        .iter()
        .flat_map(|w| w.split(','))
        .any(|w| w.trim().eq_ignore_ascii_case(word))
    {
        return None;
    }
    Some(family)
}

#[cfg(test)]
mod tests {
    use super::*;

    type Route<'a> = (&'a str, &'a [(&'a str, &'a [&'a str])]);

    fn said(sets: &[Route<'_>]) -> Vec<Said> {
        sets.iter()
            .map(|(route, axes)| Said {
                route: route.to_string(),
                axes: axes
                    .iter()
                    .map(|(a, v)| (a.to_string(), v.iter().map(|x| x.to_string()).collect()))
                    .collect(),
            })
            .collect()
    }

    fn decided(pairs: &[(&str, Option<&str>)]) -> InForce {
        pairs
            .iter()
            .map(|(a, v)| (a.to_string(), v.map(str::to_string)))
            .collect()
    }

    fn mri() -> Pack {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri");
        nils_pack::load(&dir, None).expect("the MRI pack loads")
    }

    #[test]
    fn the_family_is_the_route_that_decided_else_the_one_its_provenance_enters() {
        let none = InForce::new();
        let swi = said(&[("swi", &[("construct", &["Phase", "ND"])])]);
        assert_eq!(
            acquisition_family(None, &swi, &none, Some("RawRecon")),
            Some("swi")
        );
        assert_eq!(
            acquisition_family(None, &said(&[("epimix", &[])]), &none, Some("EPIMix")),
            Some("mix")
        );
        assert_eq!(
            acquisition_family(None, &[], &none, Some("SyMRI")),
            Some("symri")
        );
        assert_eq!(
            acquisition_family(None, &[], &none, Some("NeuroMix")),
            Some("mix")
        );
        assert_eq!(acquisition_family(None, &[], &none, Some("RawRecon")), None);
        // the route the evidence names first, not a fixed list's
        let both = said(&[("stage", &[]), ("swi", &[])]);
        assert_eq!(acquisition_family(None, &both, &none, None), Some("stage"));
        let both = said(&[("swi", &[]), ("stage", &[])]);
        assert_eq!(acquisition_family(None, &both, &none, None), Some("swi"));
    }

    #[test]
    fn a_decision_that_contradicts_a_route_takes_the_family_away() {
        let pack = mri();
        let p = Some(&pack);
        let phase = said(&[(
            "swi",
            &[
                ("construct", &["Phase", "ND"]),
                ("base", &[]),
                ("technique", &["MEGRE"]),
            ],
        )]);
        // a person corrects the construct: the phase is no SWI's any more
        let corrected = decided(&[("construct", Some("Magnitude"))]);
        assert_eq!(
            acquisition_family(p, &phase, &corrected, Some("RawRecon")),
            None
        );
        // adding a value beside the route's on a multi-valued axis is no
        // contradiction; dropping one is
        let added = decided(&[("construct", Some("ND,Phase,Real"))]);
        assert_eq!(
            acquisition_family(p, &phase, &added, Some("RawRecon")),
            Some("swi")
        );
        let dropped = decided(&[("construct", Some("Phase"))]);
        assert_eq!(
            acquisition_family(p, &phase, &dropped, Some("RawRecon")),
            None
        );
        // a single-valued axis compares as identities: ME-GRE is MEGRE
        let same = decided(&[("technique", Some("ME-GRE"))]);
        assert_eq!(
            acquisition_family(p, &phase, &same, Some("RawRecon")),
            Some("swi")
        );
        let other = decided(&[("technique", Some("GRE"))]);
        assert_eq!(
            acquisition_family(p, &phase, &other, Some("RawRecon")),
            None
        );
        // a base where the route said none contradicts it
        let based = decided(&[("base", Some("T1w"))]);
        assert_eq!(
            acquisition_family(p, &phase, &based, Some("RawRecon")),
            None
        );
        // a decision on an axis the route did not decide changes nothing
        let body = decided(&[("body_part", Some("brain"))]);
        assert_eq!(
            acquisition_family(p, &phase, &body, Some("RawRecon")),
            Some("swi")
        );
        // the provenance decided in force stands over the rules'
        let made = decided(&[("provenance", Some("SWIRecon"))]);
        assert_eq!(
            acquisition_family(p, &[], &made, Some("RawRecon")),
            Some("swi")
        );
        let unmade = decided(&[("provenance", Some("RawRecon"))]);
        assert_eq!(acquisition_family(p, &[], &unmade, Some("SyMRI")), None);
    }

    #[test]
    fn a_name_spells_the_family_where_nothing_in_it_says_it_already() {
        // an SWI's magnitude and phase: RawRecon, nothing says SWI
        assert_eq!(
            spelled(
                Some("swi"),
                Some("RawRecon"),
                None,
                &["T2*w", "GRE", "Magnitude"]
            ),
            Some("swi")
        );
        assert_eq!(
            spelled(Some("swi"), Some("RawRecon"), None, &["Phase"]),
            Some("swi")
        );
        // its processed image, `rec-SWIRecon`, and its minIP, base SWI
        assert_eq!(spelled(Some("swi"), Some("SWIRecon"), None, &["SWI"]), None);
        assert_eq!(
            spelled(
                Some("swi"),
                Some("ProjectionDerived"),
                None,
                &["SWI", "MinIP"]
            ),
            None
        );
        // SyMRI's acquisition is in SyMRI's folder, its products `rec-SyMRI`
        assert_eq!(
            spelled(Some("symri"), Some("RawRecon"), Some("SyMRI"), &["MDME"]),
            None
        );
        assert_eq!(
            spelled(Some("symri"), Some("SyMRI"), Some("SyMRI"), &["T1w"]),
            None
        );
        // what no route recognised spells nothing
        assert_eq!(spelled(None, Some("RawRecon"), None, &["T1w"]), None);
        assert_eq!(word("swi"), "SWI");
        assert_eq!(word("mix"), "Mix");
    }
}
