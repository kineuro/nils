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
use nils_registry::review;
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
    /// Record 42 S3: occasions where a person's pick stands. The run still
    /// writes its own pick there, as evidence that does not apply.
    pub standing: i64,
    /// Occasions with an open `pick.border` review item after the run.
    pub raised: i64,
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
/// A pick is a job (Wave 4a §9.1).
pub fn run(
    registry: &mut Registry,
    pack: &Pack,
    scheme: &Scheme,
    subject: Option<&str>,
    actor: &str,
) -> Result<Picked, Error> {
    let settings = crate::job::Settings {
        name: subject.unwrap_or("all").to_string(),
        ..crate::job::Settings::default()
    };
    let job_id = crate::job::claim_for(registry, &settings, "pick")?;
    let result = run_pick(registry, pack, scheme, subject, actor, Some(job_id));
    let (state, error) = match &result {
        Ok(_) => ("done", None),
        Err(e) => ("failed", Some(e.to_string())),
    };
    let _ = crate::job::finish(registry.store(), job_id, state, error.as_deref());
    result
}

fn run_pick(
    registry: &mut Registry,
    pack: &Pack,
    scheme: &Scheme,
    subject: Option<&str>,
    actor: &str,
    job_id: Option<i64>,
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

    // Wave 4b §7: the occasions come from the session cache, built over each
    // subject's whole timeline under the scheme's own anchor, and not from
    // this role's candidates, so a pick keys on the day every reader sees.
    let anchors =
        nils_session::Anchors::resolve(registry, scheme, BTreeMap::new()).map_err(session_err)?;
    nils_session::ensure(registry, scheme, &anchors, subject, false).map_err(session_err)?;
    let store = registry.store();
    let labels = nils_session::labels_by_study(store, scheme).map_err(session_err)?;
    for model in &pack.picks {
        let rows = read_rows(store, model, subject)?;
        run_one(
            store,
            model,
            pack,
            scheme,
            &labels,
            &rows,
            actor,
            job_id,
            &mut report,
        )?;
    }
    report.seconds = started.elapsed().as_secs_f64();
    Ok(report)
}

/// The day each study happened on, which is what a session is grouped by.
fn session_err(e: nils_session::Error) -> Error {
    match e {
        nils_session::Error::Store(s) => Error::Store(s),
        nils_session::Error::Message(m) => Error::Store(StoreError::Message(m)),
    }
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
        // One row per value (Wave 4a §6.1): a role arrives as rows, and an
        // axis a pick reads is joined back into the text the model matches
        // a token against.
        if axis == "role" {
            rows[i].roles.push(value.trim().to_string());
        }
        if reads.iter().any(|n| n == axis) {
            rows[i]
                .values
                .entry(axis.to_string())
                .and_modify(|v| {
                    v.push(',');
                    v.push_str(value);
                })
                .or_insert_with(|| value.to_string());
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
    labels: &HashMap<i64, nils_session::Labelled>,
    rows: &[Row],
    actor: &str,
    job_id: Option<i64>,
    report: &mut Picked,
) -> Result<(), Error> {
    let now = now_iso();
    let scheme_json = serde_json::to_string(scheme).unwrap_or_default();
    let scheme_name = short_scheme(scheme);
    let scheme_digest = scheme.digest();

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
            let mut occasions: BTreeMap<i64, (Day, Vec<&Row>)> = BTreeMap::new();
            for r in subject_rows {
                let Some(l) = labels.get(&r.study) else {
                    continue;
                };
                occasions
                    .entry(l.session_id)
                    .or_insert_with(|| (l.first, Vec::new()))
                    .1
                    .push(r);
            }
            for (first, here) in occasions.values() {
                report.sessions += 1;
                let candidates = group(model, here);
                let picked = pick::pick(model, role, &candidates, &reference);
                for b in &picked.borders {
                    *report.borders.entry(b.name().to_string()).or_insert(0) += 1;
                }
                // Record 42 S3: a person's pick on this occasion stands. The
                // run neither replaces nor withdraws it, and asks nothing
                // about an occasion a person already answered.
                let standing = standing_person(store, &model.name, role, *subject, *first)?;
                let pick_id = if picked.winner.is_none() {
                    report.empty += 1;
                    None
                } else {
                    let id = write(
                        store,
                        model,
                        pack,
                        &picked,
                        *subject,
                        *first,
                        &scheme_name,
                        &scheme_json,
                        &scheme_digest,
                        &reference.name,
                        actor,
                        &now,
                        standing,
                        job_id,
                    )?;
                    report.written += 1;
                    Some(id)
                };
                if standing.is_some() {
                    report.standing += 1;
                    continue;
                }
                if border_item(
                    store, model, &picked, *subject, *first, pick_id, actor, job_id,
                )? {
                    report.raised += 1;
                }
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
    scheme_digest: &str,
    reference: &str,
    actor: &str,
    now: &str,
    standing: Option<i64>,
    job_id: Option<i64>,
) -> Result<i64, Error> {
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
    let result = (|| -> Result<i64, StoreError> {
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
                    "scheme_digest",
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
                    "job_id",
                    "withdrawn_at",
                    "overruled_by",
                ],
            )
            .returning(&["id"]),
            &[vec![
                Param::from(model.name.as_str()),
                Param::from(picked.role.as_str()),
                Param::Int(subject),
                Param::from(day.to_string()),
                Param::from(scheme_name),
                Param::from(scheme_digest),
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
                job_id.map_or(Param::Null, Param::Int),
                // Record 42 S3: under a person's pick the run's own is kept
                // as evidence and does not apply.
                if standing.is_some() {
                    Param::from(now)
                } else {
                    Param::Null
                },
                standing.map_or(Param::Null, Param::Int),
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
        Ok(id)
    })();
    match result {
        Ok(id) => {
            store.commit()?;
            Ok(id)
        }
        Err(e) => {
            store.rollback().ok();
            Err(Error::Store(e))
        }
    }
}

/// The person's pick that stands on one role and occasion, if any.
fn standing_person(
    store: &mut Store,
    model: &str,
    role: &str,
    subject: i64,
    day: Day,
) -> Result<Option<i64>, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT id FROM {} WHERE model = {} AND role = {} AND subject_id = {} \
           AND session_day = {} AND author_kind = 'person' AND withdrawn_at IS NULL \
         ORDER BY id DESC LIMIT 1",
        store.qualified("pick"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
        d.param(3, Type::Int),
        d.param(4, Type::Date),
    );
    store
        .query_opt(
            &sql,
            &[
                Param::from(model),
                Param::from(role),
                Param::Int(subject),
                Param::from(day.to_string()),
            ],
        )?
        .map(|r| r.int(0))
        .transpose()
}

/// Record 42 S3: raise, refresh or close the `pick.border` item of one
/// occasion. Answers whether one is open after it.
#[allow(clippy::too_many_arguments)]
fn border_item(
    store: &mut Store,
    model: &Model,
    picked: &pick::Picked,
    subject: i64,
    day: Day,
    pick_id: Option<i64>,
    actor: &str,
    job_id: Option<i64>,
) -> Result<bool, StoreError> {
    let day = day.to_string();
    let key = review::pick_border_key(&model.name, &picked.role, subject, &day);
    if picked.borders.is_empty() {
        review::close_pick_border(
            store,
            &key,
            review::RESOLVED,
            actor,
            &serde_json::json!({"why": "a later run found nothing to doubt", "pick_id": pick_id}),
        )?;
        return Ok(false);
    }
    let names: Vec<&str> = picked.borders.iter().map(|b| b.name()).collect();
    let considered = serde_json::Value::Array(
        picked
            .considered
            .iter()
            .map(|(stacks, score)| serde_json::json!({"stacks": stacks, "score": score}))
            .collect(),
    );
    review::raise_pick_border(
        store,
        &review::PickBorder {
            model: &model.name,
            role: &picked.role,
            subject_id: subject,
            day: &day,
            borders: &names,
            pick_id,
            score: picked.scored.as_ref().map(|s| s.score),
            margin: picked.winner.as_ref().map(|_| picked.margin),
            runner_up_score: picked.runner_up.as_ref().map(|_| picked.runner_up_score),
            considered: &considered,
            job_id,
        },
        &now_iso(),
    )?;
    Ok(true)
}

// ------------------------------------------------------------- person picks

/// Why a person's pick or its withdrawal was not written.
#[derive(Debug)]
pub enum PersonError {
    /// The ask was wrong in a way the person can mend: said in words.
    Refused(String),
    Store(StoreError),
}

impl std::fmt::Display for PersonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PersonError::Refused(m) => write!(f, "{m}"),
            PersonError::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PersonError {}

impl From<StoreError> for PersonError {
    fn from(e: StoreError) -> Self {
        PersonError::Store(e)
    }
}

fn refused(m: impl Into<String>) -> PersonError {
    PersonError::Refused(m.into())
}

/// A person's pick (record 42 S3): the stacks that stand for a role on the
/// occasion they belong to, and why.
#[derive(Debug, Clone)]
pub struct PersonPick<'a> {
    pub role: &'a str,
    /// One acquisition: every stack the pick names, on one occasion of one
    /// subject.
    pub stacks: &'a [i64],
    /// The pack's pick that declares the role; the only one when omitted.
    pub model: Option<&'a str>,
    pub why: &'a str,
    /// The principal, as every provenance writer names it.
    pub actor: &'a str,
}

/// What a person's pick wrote.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PersonPicked {
    pub id: i64,
    pub model: String,
    pub role: String,
    pub subject_id: i64,
    pub session_day: String,
    pub stacks: Vec<i64>,
    /// The run's picks that stopped applying under it.
    pub overruled: Vec<i64>,
    /// An earlier person's pick on the same occasion it replaced.
    pub replaced: Vec<i64>,
    /// The `pick.border` items it answered.
    pub answered: u64,
}

/// Write a person's pick. It stands until a person withdraws it: a pick run
/// on the same occasion keeps its own pick as evidence that does not apply.
/// An earlier person's pick on the occasion is withdrawn by this one, and a
/// run's pick stops applying, pointing at this one, so that withdrawing it
/// lets the run's pick apply again.
pub fn set_person(
    registry: &mut Registry,
    pack: &Pack,
    scheme: &Scheme,
    p: &PersonPick<'_>,
) -> Result<PersonPicked, PersonError> {
    use nils_registry::audit::{self, Action, Entry};
    use nils_registry::review;

    if p.stacks.is_empty() {
        return Err(refused("a pick names at least one stack"));
    }
    if p.why.trim().is_empty() {
        return Err(refused(
            "a person's pick says why; it is what a reader of the pick has in place of the run's scores",
        ));
    }
    let declaring: Vec<&Model> = pack
        .picks
        .iter()
        .filter(|m| m.roles.iter().any(|r| r == p.role))
        .filter(|m| p.model.is_none_or(|n| n == m.name))
        .collect();
    let model = match declaring.as_slice() {
        [one] => *one,
        [] => {
            return Err(refused(format!(
                "{} declares no pick for the role {}{}",
                pack.id(),
                p.role,
                p.model.map(|m| format!(" in {m}")).unwrap_or_default()
            )));
        }
        _ => {
            return Err(refused(format!(
                "more than one pick of {} declares the role {}; name the pick",
                pack.id(),
                p.role
            )));
        }
    };

    // Where each stack sits: its subject and its study, and through the
    // scheme the occasion the study belongs to.
    let mut stacks: Vec<i64> = p.stacks.to_vec();
    stacks.sort_unstable();
    stacks.dedup();
    let store = registry.store();
    let list = stacks
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT f.stack_id, f.subject_id, f.study_id, su.code FROM {} f \
         JOIN {} su ON su.id = f.subject_id WHERE f.stack_id IN ({list})",
        store.qualified("stack_fingerprint"),
        store.qualified("subject"),
    );
    let rows = store.query(&sql, &[])?;
    if rows.len() != stacks.len() {
        let found: Vec<i64> = rows.iter().filter_map(|r| r.int(0).ok()).collect();
        let missing: Vec<String> = stacks
            .iter()
            .filter(|s| !found.contains(s))
            .map(i64::to_string)
            .collect();
        return Err(refused(format!(
            "no fingerprinted stack {}; a pick names stacks the fingerprint has read",
            missing.join(", ")
        )));
    }
    let subject = rows[0].int(1)?;
    let code = rows[0].text(3)?.to_string();
    if rows.iter().any(|r| r.int(1).ok() != Some(subject)) {
        return Err(refused("the stacks of one pick belong to one subject"));
    }
    let studies: Vec<i64> = rows.iter().filter_map(|r| r.int(2).ok()).collect();

    let anchors = nils_session::Anchors::resolve(registry, scheme, BTreeMap::new())
        .map_err(|e| refused(session_err(e).to_string()))?;
    nils_session::ensure(registry, scheme, &anchors, Some(&code), false)
        .map_err(|e| refused(session_err(e).to_string()))?;
    let store = registry.store();
    let labels = nils_session::labels_by_study(store, scheme)
        .map_err(|e| refused(session_err(e).to_string()))?;
    let mut occasions: Vec<(i64, Day)> = Vec::new();
    for study in &studies {
        let Some(l) = labels.get(study) else {
            return Err(refused(
                "a stack of the pick is on no session under the scheme; rebuild the sessions first",
            ));
        };
        if !occasions.iter().any(|(id, _)| *id == l.session_id) {
            occasions.push((l.session_id, l.first));
        }
    }
    let [(_, day)] = occasions.as_slice() else {
        return Err(refused(
            "the stacks of one pick are on one occasion; these are on more than one under the scheme",
        ));
    };
    let day = *day;
    let day_text = day.to_string();
    let now = now_iso();
    let scheme_json = serde_json::to_string(scheme).unwrap_or_default();

    store.begin()?;
    let written = (|| -> Result<PersonPicked, StoreError> {
        let d = store.dialect();
        let key = [
            Param::from(model.name.as_str()),
            Param::from(p.role),
            Param::Int(subject),
            Param::from(day_text.as_str()),
        ];
        let on_key = format!(
            "model = {} AND role = {} AND subject_id = {} AND session_day = {}",
            d.param(1, Type::Text),
            d.param(2, Type::Text),
            d.param(3, Type::Int),
            d.param(4, Type::Date),
        );
        let ids = |store: &mut Store, filter: &str| -> Result<Vec<i64>, StoreError> {
            let sql = format!(
                "SELECT id FROM {} WHERE {on_key} AND {filter} ORDER BY id",
                store.qualified("pick")
            );
            store.query(&sql, &key)?.iter().map(|r| r.int(0)).collect()
        };
        let replaced = ids(store, "author_kind = 'person' AND withdrawn_at IS NULL")?;
        let overruled = ids(store, "author_kind <> 'person' AND withdrawn_at IS NULL")?;

        let written = store.insert(
            &Insert::new(
                table("pick"),
                &[
                    "model",
                    "role",
                    "subject_id",
                    "session_day",
                    "scheme",
                    "scheme_digest",
                    "reference",
                    "pack",
                    "pack_version",
                    "actor",
                    "author_kind",
                    "decided_at",
                    "why",
                    "parts",
                ],
            )
            .returning(&["id"]),
            &[vec![
                Param::from(model.name.as_str()),
                Param::from(p.role),
                Param::Int(subject),
                Param::from(day_text.as_str()),
                Param::from(short_scheme(scheme)),
                Param::from(scheme.digest()),
                // A person's pick is scored against nothing: the population
                // is the run's word, and this one is a judgement.
                Param::from("person"),
                Param::from(pack.name.as_str()),
                Param::from(pack.version.to_string()),
                Param::from(p.actor),
                Param::from("person"),
                Param::from(now.as_str()),
                Param::from(p.why),
                Param::from(
                    serde_json::json!({
                        "scheme": serde_json::from_str::<serde_json::Value>(&scheme_json)
                            .unwrap_or(serde_json::Value::Null),
                    })
                    .to_string(),
                ),
            ]],
        )?;
        let id = written
            .first()
            .map(|r| r.int(0))
            .transpose()?
            .ok_or_else(|| StoreError::Message("the pick was not written back".into()))?;
        store.insert(
            &Insert::new(table("pick_stack"), &["pick_id", "stack_id"]),
            &stacks
                .iter()
                .map(|s| vec![Param::Int(id), Param::Int(*s)])
                .collect::<Vec<_>>(),
        )?;
        // An earlier person's pick is withdrawn by this one, and a run's
        // pick it had overruled now points here.
        for old in &replaced {
            store.update_by_id(
                table("pick"),
                &[
                    ("withdrawn_at", Param::from(now.as_str())),
                    ("withdrawn_by", Param::from(p.actor)),
                ],
                "id",
                *old,
            )?;
            let sql = format!(
                "UPDATE {} SET overruled_by = {} WHERE overruled_by = {}",
                store.qualified("pick"),
                d.param(1, Type::Int),
                d.param(2, Type::Int),
            );
            store.execute(&sql, &[Param::Int(id), Param::Int(*old)])?;
        }
        for run in &overruled {
            store.update_by_id(
                table("pick"),
                &[
                    ("withdrawn_at", Param::from(now.as_str())),
                    ("overruled_by", Param::Int(id)),
                ],
                "id",
                *run,
            )?;
        }
        let answered = review::close_pick_border(
            store,
            &review::pick_border_key(&model.name, p.role, subject, &day_text),
            "accepted",
            p.actor,
            &serde_json::json!({
                "pick_id": id, "stacks": stacks, "author_kind": "person", "actor": p.actor, "why": p.why,
            }),
        )?;
        Ok(PersonPicked {
            id,
            model: model.name.clone(),
            role: p.role.to_string(),
            subject_id: subject,
            session_day: day_text.clone(),
            stacks: stacks.clone(),
            overruled,
            replaced,
            answered,
        })
    })();
    let picked = match written {
        Ok(w) => w,
        Err(e) => {
            store.rollback().ok();
            return Err(e.into());
        }
    };
    store.commit()?;
    audit::record(
        registry,
        &Entry {
            principal: p.actor,
            action: Action::PickSet,
            scope: serde_json::json!({
                "pick": picked.id, "model": picked.model, "role": picked.role,
                "subject_id": picked.subject_id, "stacks": picked.stacks,
                "overruled": picked.overruled, "replaced": picked.replaced,
            }),
            policy: None,
            job_id: None,
            details: Some(serde_json::json!({ "why": p.why })),
        },
    )?;
    Ok(picked)
}

/// What withdrawing a person's pick did.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PersonWithdrawn {
    pub id: i64,
    /// The run's picks that apply again.
    pub restored: Vec<i64>,
}

/// Withdraw a person's pick. The row stays and stops applying, and the
/// run's pick it overruled applies again. A run's pick is not withdrawn:
/// a person overrules it with a pick of their own.
pub fn withdraw_person(
    registry: &mut Registry,
    id: i64,
    actor: &str,
    why: Option<&str>,
) -> Result<PersonWithdrawn, PersonError> {
    use nils_registry::audit::{self, Action, Entry};

    let store = registry.store();
    let d = store.dialect();
    let sql = format!(
        "SELECT author_kind, withdrawn_at IS NOT NULL FROM {} WHERE id = {}",
        store.qualified("pick"),
        d.param(1, Type::Int)
    );
    let Some(row) = store.query_opt(&sql, &[Param::Int(id)])? else {
        return Err(refused(format!("no pick {id}")));
    };
    if row.text(0)? != "person" {
        return Err(refused(format!(
            "pick {id} is a run's; a person overrules it with a pick of their own (nils pick set), and the next run keeps that"
        )));
    }
    if row.int(1)? != 0 {
        return Err(refused(format!("pick {id} is already withdrawn")));
    }
    let now = now_iso();
    store.begin()?;
    let written = (|| -> Result<Vec<i64>, StoreError> {
        store.update_by_id(
            table("pick"),
            &[
                ("withdrawn_at", Param::from(now.as_str())),
                ("withdrawn_by", Param::from(actor)),
            ],
            "id",
            id,
        )?;
        let sql = format!(
            "SELECT id FROM {} WHERE overruled_by = {} ORDER BY id",
            store.qualified("pick"),
            d.param(1, Type::Int)
        );
        let restored: Vec<i64> = store
            .query(&sql, &[Param::Int(id)])?
            .iter()
            .map(|r| r.int(0))
            .collect::<Result<_, _>>()?;
        let sql = format!(
            "UPDATE {} SET withdrawn_at = NULL, overruled_by = NULL WHERE overruled_by = {}",
            store.qualified("pick"),
            d.param(1, Type::Int)
        );
        store.execute(&sql, &[Param::Int(id)])?;
        Ok(restored)
    })();
    let restored = match written {
        Ok(r) => r,
        Err(e) => {
            store.rollback().ok();
            return Err(e.into());
        }
    };
    store.commit()?;
    audit::record(
        registry,
        &Entry {
            principal: actor,
            action: Action::PickWithdraw,
            scope: serde_json::json!({ "pick": id, "restored": restored }),
            policy: None,
            job_id: None,
            details: why.map(|w| serde_json::json!({ "why": w })),
        },
    )?;
    Ok(PersonWithdrawn { id, restored })
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
