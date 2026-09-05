// SPDX-License-Identifier: AGPL-3.0-only

//! Running a pack's picks over a registry
//! (`docs/specs/wave3-anonymize-and-bids.md`, §10).
//!
//! The model and the scoring are `nils_pack`'s, so a pack can be checked
//! against a fixture by somebody who has never seen this schema. What is here
//! is the registry: which rows to read, how a session is derived, what the
//! population is, and what is written down.
//!
//! Three things this does that v0 does not.
//!
//! It says which **population** the cohort-relative components were scored
//! against. Three of the eight read one, so the same stack scored against two
//! cohorts gets two answers; v0 records neither the population nor the fact
//! that it read one, so its picks cannot be reproduced from what is stored.
//!
//! It **reports a tie** instead of settling it by row order. v0 sorts its
//! bundles and takes the first, so a session whose two best differ by nothing
//! gets whichever the database returned, and the same cohort re-run can return
//! the other.
//!
//! And it names the **scheme** the session came from, because a session is
//! derived (§5) and the same studies are one occasion or two depending on it.

use std::collections::{BTreeMap, HashMap};

use nils_pack::pack::Pack;
use nils_pack::pick::{self, Candidate, Model, Reference};
use nils_registry::schema::{Type, table};
use nils_registry::session::{self, Scheme};
use nils_registry::store::{Error as StoreError, Insert, Param, Store};
use nils_registry::{Registry, day::Day, time::now_iso};

use crate::Error;

/// What a run of the picks says when it is done.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Picked {
    /// Occasions looked at, per role.
    pub sessions: i64,
    /// Picks written.
    pub written: i64,
    /// Of those, the ones a person should look at, by reason.
    pub borders: BTreeMap<String, i64>,
    /// Occasions where the role had no candidate at all.
    pub empty: i64,
    /// The population each role was scored against.
    pub reference: String,
    pub seconds: f64,
}

/// One stack, as the picks need it.
struct Row {
    stack: i64,
    subject: i64,
    study: i64,
    values: BTreeMap<String, String>,
    roles: Vec<String>,
}

/// Run every pick the pack declares.
pub fn run(
    registry: &mut Registry,
    pack: &Pack,
    scheme: &Scheme,
    subject: Option<&str>,
    actor: &str,
) -> Result<Picked, Error> {
    let started = std::time::Instant::now();
    let mut report = Picked::default();
    if pack.picks.is_empty() {
        return Ok(report);
    }
    report.reference = match subject {
        Some(code) => format!("subject:{code}"),
        None => "registry".to_string(),
    };

    let store = registry.store();
    let days = study_days(store)?;
    for model in &pack.picks {
        let rows = read_rows(store, model, subject)?;
        run_one(store, model, pack, scheme, &days, &rows, actor, &mut report)?;
    }
    report.seconds = started.elapsed().as_secs_f64();
    Ok(report)
}

/// The day each study happened on, which is what a session is grouped by.
fn study_days(store: &mut Store) -> Result<HashMap<i64, Day>, Error> {
    let sql = format!(
        "SELECT id, COALESCE(date_filled, study_date) FROM {} \
         WHERE COALESCE(date_filled, study_date) IS NOT NULL",
        store.qualified("study")
    );
    let mut out = HashMap::new();
    for r in store.query(&sql, &[])? {
        if let Some(day) = Day::parse(r.text(1)?) {
            out.insert(r.int(0)?, day);
        }
    }
    Ok(out)
}

/// Every stack that holds a role, with the values this model reads.
fn read_rows(store: &mut Store, model: &Model, subject: Option<&str>) -> Result<Vec<Row>, Error> {
    let reads = model.reads();
    // A name is either a fingerprint column or an axis. The fingerprint half
    // is read in one pass over the table; the axes in one pass over theirs.
    let t = table("stack_fingerprint");
    let dialect = store.dialect();
    let mut columns = vec![
        "f.stack_id".to_string(),
        "f.subject_id".to_string(),
        "f.study_id".to_string(),
    ];
    let mut fields: Vec<String> = Vec::new();
    for name in &reads {
        let Some((_, column)) = crate::classify::FIELDS.iter().find(|(n, _)| n == name) else {
            continue;
        };
        let c = t
            .column(column)
            .unwrap_or_else(|| panic!("stack_fingerprint.{column} is not a column"));
        columns.push(dialect.text_of_qualified(Some("f"), c));
        fields.push(name.clone());
    }
    let filter = match subject {
        Some(code) => format!(
            " JOIN {} su ON su.id = f.subject_id AND su.code = '{}'",
            store.qualified("subject"),
            code.replace('\'', "''")
        ),
        None => String::new(),
    };
    let sql = format!(
        "SELECT {} FROM {} f{filter} ORDER BY f.stack_id",
        columns.join(", "),
        store.qualified("stack_fingerprint"),
    );
    let mut rows: Vec<Row> = Vec::new();
    for r in store.query(&sql, &[])? {
        let mut values = BTreeMap::new();
        for (i, name) in fields.iter().enumerate() {
            if let Some(v) = crate::classify::cell_text(r.get(i + 3))
                && !v.is_empty()
            {
                values.insert(name.clone(), v);
            }
        }
        rows.push(Row {
            stack: r.int(0)?,
            subject: r.int(1)?,
            study: r.int(2)?,
            values,
            roles: Vec::new(),
        });
    }

    // The axes, including `role`, which is what says a stack is a candidate.
    let sql = format!(
        "SELECT stack_id, axis, value FROM {}",
        store.qualified("classification_axis")
    );
    let mut by_stack: HashMap<i64, usize> = HashMap::new();
    for (i, r) in rows.iter().enumerate() {
        by_stack.insert(r.stack, i);
    }
    for r in store.query(&sql, &[])? {
        let Some(i) = by_stack.get(&r.int(0)?).copied() else {
            continue;
        };
        let axis = r.text(1)?;
        let Some(value) = r.opt_text(2)? else {
            continue;
        };
        if axis == "role" {
            rows[i].roles = value.split(',').map(|v| v.trim().to_string()).collect();
        }
        if reads.iter().any(|n| n == axis) {
            rows[i].values.insert(axis.to_string(), value.to_string());
        }
    }
    rows.retain(|r| !r.roles.is_empty());
    Ok(rows)
}

#[allow(clippy::too_many_arguments)]
fn run_one(
    store: &mut Store,
    model: &Model,
    pack: &Pack,
    scheme: &Scheme,
    days: &HashMap<i64, Day>,
    rows: &[Row],
    actor: &str,
    report: &mut Picked,
) -> Result<(), Error> {
    let now = now_iso();
    let scheme_json = serde_json::to_string(scheme).unwrap_or_default();
    let scheme_name = short_scheme(scheme);

    for role in &model.roles {
        let mine: Vec<&Row> = rows
            .iter()
            .filter(|r| r.roles.iter().any(|x| x == role))
            .collect();
        // The population is every candidate for this role, and it is named on
        // every row it decided.
        let reference = build_reference(model, &report.reference, &mine);

        // Which studies are one occasion, per subject.
        let mut by_subject: BTreeMap<i64, Vec<&Row>> = BTreeMap::new();
        for r in &mine {
            by_subject.entry(r.subject).or_default().push(r);
        }
        for (subject, subject_rows) in &by_subject {
            let mut studies: Vec<session::Study> = Vec::new();
            let mut seen: Vec<i64> = Vec::new();
            for r in subject_rows {
                if seen.contains(&r.study) {
                    continue;
                }
                let Some(day) = days.get(&r.study) else {
                    continue;
                };
                seen.push(r.study);
                studies.push(session::Study::new(r.study, *day));
            }
            if studies.is_empty() {
                continue;
            }
            let anchor = studies.iter().map(|s| s.day).min();
            for occasion in session::sessions(&studies, anchor, scheme) {
                report.sessions += 1;
                let here: Vec<&Row> = subject_rows
                    .iter()
                    .copied()
                    .filter(|r| occasion.studies.contains(&r.study))
                    .collect();
                let candidates = group(model, &here);
                let picked = pick::pick(model, role, &candidates, &reference);
                for b in &picked.borders {
                    *report.borders.entry(b.name().to_string()).or_insert(0) += 1;
                }
                if picked.winner.is_none() {
                    report.empty += 1;
                    continue;
                }
                write(
                    store,
                    model,
                    pack,
                    &picked,
                    *subject,
                    occasion.first,
                    &scheme_name,
                    &scheme_json,
                    &reference.name,
                    actor,
                    &now,
                )?;
                report.written += 1;
            }
        }
    }
    Ok(())
}

/// A short name for the scheme, so a row says which one it was made under
/// without carrying the whole of it.
fn short_scheme(scheme: &Scheme) -> String {
    let naming = match &scheme.naming {
        session::Naming::Date => "date".to_string(),
        session::Naming::Ordinal => "ordinal".to_string(),
        session::Naming::Months { cadence, tolerance } => format!(
            "months[{}]+-{tolerance}",
            cadence
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
    };
    format!("window={}d,{naming}", scheme.window_days)
}

/// Two stacks of one acquisition are one candidate, and the outputs of one
/// acquisition are merged back into one after that.
fn group(model: &Model, rows: &[&Row]) -> Vec<Candidate> {
    let key_of = |r: &Row, ignoring: &[String], over: Option<&str>| -> String {
        model
            .same_acquisition
            .iter()
            .filter(|n| Some(n.as_str()) != over && !ignoring.contains(n))
            .map(|n| r.values.get(n).cloned().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\u{1}")
    };

    // Stage one: the full key.
    let mut groups: BTreeMap<String, Vec<&Row>> = BTreeMap::new();
    for r in rows {
        groups.entry(key_of(r, &[], None)).or_default().push(r);
    }

    // Stage two: the outputs of one acquisition, merged. Only where the
    // family's token is held, and only where more than one output exists;
    // a lone in-phase image is one acquisition either way.
    let mut merged: BTreeMap<String, Vec<&Row>> = BTreeMap::new();
    if let Some(family) = &model.family {
        let mut families: BTreeMap<String, Vec<&Row>> = BTreeMap::new();
        for (key, members) in groups {
            let is_family = members.first().is_some_and(|r| {
                r.values
                    .get(&family.when.0)
                    .is_some_and(|v| holds(v, &family.when.1))
            });
            if !is_family {
                merged.insert(key, members);
                continue;
            }
            let fkey = key_of(members[0], &family.ignoring, Some(&family.over));
            families.entry(fkey).or_default().extend(members);
        }
        for (key, members) in families {
            merged.insert(format!("family\u{1}{key}"), members);
        }
    } else {
        merged = groups;
    }

    let mut out = Vec::new();
    for members in merged.into_values() {
        // Within a family, only the variants worth keeping. A family with
        // none of them is not a candidate at all.
        let kept: Vec<&&Row> = match &model.family {
            Some(f) if members.len() > 1 && is_family(&members, f) => {
                let mut kept: Vec<&&Row> = Vec::new();
                for want in &f.canonical {
                    kept = members
                        .iter()
                        .filter(|r| r.values.get(&f.over).is_some_and(|v| holds(v, want)))
                        .collect();
                    if !kept.is_empty() {
                        break;
                    }
                }
                if kept.is_empty() {
                    continue;
                }
                kept
            }
            _ => members.iter().collect(),
        };

        // The values of the group: what they agree on, and each number at its
        // largest, because a bundle's slice count is the fullest volume in it.
        let mut values: BTreeMap<String, String> = BTreeMap::new();
        for name in model.reads() {
            let mut best: Option<String> = None;
            for r in &kept {
                let Some(v) = r.values.get(&name) else {
                    continue;
                };
                best = Some(match (best, v.parse::<f64>(), v) {
                    (None, _, v) => v.clone(),
                    (Some(b), Ok(n), v) => match b.parse::<f64>() {
                        Ok(m) if n > m => v.clone(),
                        Ok(_) => b,
                        Err(_) => b,
                    },
                    (Some(b), Err(_), _) => b,
                });
            }
            if let Some(v) = best {
                values.insert(name, v);
            }
        }
        let mut stacks: Vec<i64> = kept.iter().map(|r| r.stack).collect();
        stacks.sort_unstable();
        out.push(Candidate { stacks, values });
    }
    out
}

fn is_family(members: &[&Row], f: &nils_pack::pick::Family) -> bool {
    members
        .first()
        .is_some_and(|r| r.values.get(&f.when.0).is_some_and(|v| holds(v, &f.when.1)))
}

fn holds(csv: &str, token: &str) -> bool {
    csv.split(',').any(|t| t.trim().eq_ignore_ascii_case(token))
}

/// What the population says about itself.
fn build_reference(model: &Model, name: &str, rows: &[&Row]) -> Reference {
    let mut counts: BTreeMap<String, BTreeMap<String, i64>> = BTreeMap::new();
    for name in model.reads() {
        let mut per: BTreeMap<String, i64> = BTreeMap::new();
        for r in rows {
            if let Some(v) = r.values.get(&name)
                && !v.is_empty()
            {
                *per.entry(v.clone()).or_insert(0) += 1;
            }
        }
        if !per.is_empty() {
            counts.insert(name, per);
        }
    }
    let mut percentiles = BTreeMap::new();
    for (population, of, split_by) in model.populations() {
        let mut buckets: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        for r in rows {
            let Some(v) = r.values.get(&of).and_then(|v| v.parse::<f64>().ok()) else {
                continue;
            };
            let key = match &split_by {
                Some(s) => format!(
                    "{population}:{}",
                    r.values.get(s).cloned().unwrap_or_default()
                ),
                None => population.clone(),
            };
            buckets.entry(key).or_default().push(v);
        }
        for (key, values) in buckets {
            if let Some(p) = Reference::of(&values) {
                percentiles.insert(key, p);
            }
        }
    }
    Reference {
        name: name.to_string(),
        counts,
        percentiles,
        total: rows.len() as i64,
    }
}

#[allow(clippy::too_many_arguments)]
fn write(
    store: &mut Store,
    model: &Model,
    pack: &Pack,
    picked: &pick::Picked,
    subject: i64,
    day: Day,
    scheme_name: &str,
    scheme_json: &str,
    reference: &str,
    actor: &str,
    now: &str,
) -> Result<(), Error> {
    let winner = picked.winner.as_ref().expect("a pick with a winner");
    let scored = picked.scored.as_ref().expect("a winner is scored");
    let parts = serde_json::json!({
        "scheme": serde_json::from_str::<serde_json::Value>(scheme_json)
            .unwrap_or(serde_json::Value::Null),
        "penalty": scored.penalty,
        "parts": scored
            .parts
            .iter()
            .map(|p| serde_json::json!({
                "name": p.name, "score": p.score, "weight": p.weight, "saw": p.saw,
            }))
            .collect::<Vec<_>>(),
    });
    let considered: Vec<serde_json::Value> = picked
        .considered
        .iter()
        .map(|(stacks, score)| serde_json::json!({"stacks": stacks, "score": score}))
        .collect();
    let borders: Vec<&str> = picked.borders.iter().map(|b| b.name()).collect();

    store.begin()?;
    let result = (|| -> Result<(), StoreError> {
        // A run replaces what it decided before for this role and occasion.
        // What a person decided is a withdrawal, not a row this can reach.
        let d = store.dialect();
        let sql = format!(
            "DELETE FROM {} WHERE pick_id IN (SELECT id FROM {} \
             WHERE model = {} AND role = {} AND subject_id = {} AND session_day = {} \
               AND author_kind = 'agent')",
            store.qualified("pick_stack"),
            store.qualified("pick"),
            d.param(1, Type::Text),
            d.param(2, Type::Text),
            d.param(3, Type::Int),
            d.param(4, Type::Date),
        );
        let key = [
            Param::from(model.name.as_str()),
            Param::from(picked.role.as_str()),
            Param::Int(subject),
            Param::from(day.to_string()),
        ];
        store.execute(&sql, &key)?;
        let sql = format!(
            "DELETE FROM {} WHERE model = {} AND role = {} AND subject_id = {} \
               AND session_day = {} AND author_kind = 'agent'",
            store.qualified("pick"),
            d.param(1, Type::Text),
            d.param(2, Type::Text),
            d.param(3, Type::Int),
            d.param(4, Type::Date),
        );
        store.execute(&sql, &key)?;

        let written = store.insert(
            &Insert::new(
                table("pick"),
                &[
                    "model",
                    "role",
                    "subject_id",
                    "session_day",
                    "scheme",
                    "score",
                    "margin",
                    "runner_up_score",
                    "borders",
                    "parts",
                    "considered",
                    "reference",
                    "pack",
                    "pack_version",
                    "actor",
                    "author_kind",
                    "decided_at",
                ],
            )
            .returning(&["id"]),
            &[vec![
                Param::from(model.name.as_str()),
                Param::from(picked.role.as_str()),
                Param::Int(subject),
                Param::from(day.to_string()),
                Param::from(scheme_name),
                Param::Double(scored.score),
                Param::Double(picked.margin),
                Param::Double(picked.runner_up_score),
                if borders.is_empty() {
                    Param::Null
                } else {
                    Param::from(borders.join(","))
                },
                Param::from(parts.to_string()),
                Param::from(serde_json::Value::Array(considered).to_string()),
                Param::from(reference),
                Param::from(pack.name.as_str()),
                Param::from(pack.version.to_string()),
                Param::from(actor),
                // §10.1: an automatic pick is an agent's, and says so, so that
                // a person's call is distinguishable from it wherever it is
                // read.
                Param::from("agent"),
                Param::from(now),
            ]],
        )?;
        let id = written.first().map(|r| r.int(0)).transpose()?.unwrap_or(0);
        store.insert(
            &Insert::new(table("pick_stack"), &["pick_id", "stack_id"]),
            &winner
                .stacks
                .iter()
                .map(|s| vec![Param::Int(id), Param::Int(*s)])
                .collect::<Vec<_>>(),
        )?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            store.commit()?;
            Ok(())
        }
        Err(e) => {
            store.rollback().ok();
            Err(Error::Store(e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nils_pack::pick::{Borders, Component, Family, Kind};

    fn row(stack: i64, pairs: &[(&str, &str)]) -> Row {
        Row {
            stack,
            subject: 1,
            study: 1,
            values: pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            roles: vec!["t1w".into()],
        }
    }

    fn model(family: Option<Family>) -> Model {
        Model {
            name: "main".into(),
            roles: vec!["t1w".into()],
            components: vec![Component {
                name: "slices".into(),
                weight: 1.0,
                kind: Kind::Percentile {
                    of: "n_instances".into(),
                    population: "slices".into(),
                    split_by: None,
                    missing: 0.4,
                    unknown: 0.6,
                },
            }],
            penalty: None,
            borders: Borders {
                runner_up_within: 0.05,
                rare_within: None,
            },
            same_acquisition: vec![
                "technique".into(),
                "modifier".into(),
                "construct".into(),
                "echo_time".into(),
            ],
            family,
        }
    }

    fn dixon() -> Family {
        Family {
            when: ("modifier".into(), "Dixon".into()),
            over: "construct".into(),
            ignoring: vec!["echo_time".into()],
            canonical: vec!["InPhase".into(), "Water".into()],
        }
    }

    #[test]
    fn two_stacks_of_one_acquisition_are_one_candidate() {
        let m = model(None);
        let a = row(1, &[("technique", "MPRAGE"), ("echo_time", "2.3")]);
        let b = row(2, &[("technique", "MPRAGE"), ("echo_time", "2.3")]);
        let c = row(3, &[("technique", "MPRAGE"), ("echo_time", "4.6")]);
        let got = group(&m, &[&a, &b, &c]);
        assert_eq!(got.len(), 2, "the two echoes are two acquisitions");
        let together: Vec<&Vec<i64>> = got.iter().map(|c| &c.stacks).collect();
        assert!(together.contains(&&vec![1, 2]));
        assert!(together.contains(&&vec![3]));
    }

    #[test]
    fn the_outputs_of_one_dixon_do_not_compete_with_each_other() {
        // Four images of one acquisition. Without the family merge they are
        // four candidates for the session, and the pick is between them.
        let m = model(Some(dixon()));
        let of = |stack, construct, te| {
            row(
                stack,
                &[
                    ("technique", "VIBE"),
                    ("modifier", "Dixon"),
                    ("construct", construct),
                    ("echo_time", te),
                ],
            )
        };
        let w = of(1, "Water", "2.3");
        let f = of(2, "Fat", "2.4");
        let i = of(3, "InPhase", "2.5");
        let o = of(4, "OutPhase", "2.6");
        let got = group(&m, &[&w, &f, &i, &o]);
        assert_eq!(got.len(), 1, "one acquisition, one candidate");
        // And of its outputs only the canonical one, best first: v0's order is
        // in-phase then water, and a family is judged on what a reader would
        // actually measure on.
        assert_eq!(got[0].stacks, [3]);
    }

    #[test]
    fn a_family_with_nothing_worth_keeping_is_not_a_candidate() {
        // v0 drops it, and its comment says why: a Dixon with neither an
        // in-phase nor a water image is not a T1w anybody measures on.
        let m = model(Some(dixon()));
        let f = row(
            1,
            &[
                ("technique", "VIBE"),
                ("modifier", "Dixon"),
                ("construct", "Fat"),
                ("echo_time", "2.3"),
            ],
        );
        let o = row(
            2,
            &[
                ("technique", "VIBE"),
                ("modifier", "Dixon"),
                ("construct", "OutPhase"),
                ("echo_time", "2.4"),
            ],
        );
        assert!(group(&m, &[&f, &o]).is_empty());
    }

    #[test]
    fn a_lone_output_is_one_acquisition_either_way() {
        let m = model(Some(dixon()));
        let w = row(
            1,
            &[
                ("technique", "VIBE"),
                ("modifier", "Dixon"),
                ("construct", "Water"),
                ("echo_time", "2.3"),
            ],
        );
        let got = group(&m, &[&w]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].stacks, [1]);
    }

    #[test]
    fn a_candidate_takes_each_number_at_its_largest() {
        // A bundle's slice count is the fullest volume in it, not whichever
        // one the database returned first.
        let m = model(None);
        let a = row(
            1,
            &[
                ("technique", "MPRAGE"),
                ("echo_time", "2.3"),
                ("n_instances", "40"),
            ],
        );
        let b = row(
            2,
            &[
                ("technique", "MPRAGE"),
                ("echo_time", "2.3"),
                ("n_instances", "176"),
            ],
        );
        let got = group(&m, &[&a, &b]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].get("n_instances"), "176");
    }

    #[test]
    fn a_population_is_what_the_candidates_of_that_role_are() {
        let m = model(None);
        let a = row(1, &[("technique", "MPRAGE"), ("n_instances", "176")]);
        let b = row(2, &[("technique", "MPRAGE"), ("n_instances", "160")]);
        let c = row(3, &[("technique", "TSE"), ("n_instances", "40")]);
        let r = build_reference(&m, "registry", &[&a, &b, &c]);
        assert_eq!(r.total, 3);
        assert!((r.share("technique", "MPRAGE") - 2.0 / 3.0).abs() < 1e-9);
        assert!((r.share("technique", "TSE") - 1.0 / 3.0).abs() < 1e-9);
        // Three values is the floor for a bucket, which is v0's and is kept:
        // a bucket drawn from two numbers says more about the two than about
        // the population.
        assert!(r.percentiles.contains_key("slices"));
        let two = build_reference(&m, "registry", &[&a, &b]);
        assert!(two.percentiles.is_empty());
    }

    #[test]
    fn a_scheme_is_named_on_the_row_because_it_decides_what_an_occasion_is() {
        let same_day = short_scheme(&Scheme::default());
        let fortnight = short_scheme(&Scheme {
            window_days: 14,
            ..Scheme::default()
        });
        assert_ne!(same_day, fortnight);
        assert!(fortnight.contains("14d"), "{fortnight}");
    }
}
