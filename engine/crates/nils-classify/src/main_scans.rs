// SPDX-License-Identifier: AGPL-3.0-only

//! The Main scans page's map (record 55, decision 6; the main-scans study,
//! section 5): what a scope's rules, saved or drafted, take for one role,
//! read from the stored axes and written nowhere. The kinds the scope's
//! visits hold and take, where they come from (kinds by scanner or by
//! dataset), how each subject's series of kinds runs, visit by visit, and
//! what a draft changes against the saved rules. A person's pick counts on
//! both sides.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;

use nils_pack::pick::{Candidate, Model, Reference};
use nils_pack::pick_rules::{self, Chooser, Chosen, Rules};
use nils_registry::pick_truth::Of;
use nils_registry::schema::{Type, table};
use nils_registry::session::Scheme;
use nils_registry::store::{Error as StoreError, Param, Store};
use nils_registry::{Registry, day::Day};
use serde_json::{Value, json};

use crate::Error;
use crate::picking::{self, Row};

/// The scanner and field names a visit's column reads, beside the pick's.
const COLUMNS: &[&str] = &[
    "manufacturer",
    "manufacturer_model_name",
    "field_strength_normalized",
];

/// How many columns are named; the rest are one.
const NAMED_COLUMNS: usize = 8;

/// What the map reads: one role of a scope's subjects.
pub struct Ask<'a> {
    pub model: &'a Model,
    pub role: &'a str,
    pub scheme: &'a Scheme,
    pub subjects: &'a BTreeSet<i64>,
    /// Whose stored picks are read (a person's picks among them), as the
    /// one resolver says (`nils_registry::pick_truth`): the scope's own once
    /// read, else the pack's.
    pub picks: &'a Of,
    /// The datasets whose folders hold a file of each stack, by name, for
    /// columns by dataset. A stack copied into two datasets is one stack
    /// with a path in each, and is in both (Nima, 2026-10-12: a dataset is
    /// a folder that holds stacks).
    pub datasets: &'a HashMap<i64, Vec<String>>,
    /// Stacks the caller is not shown (of a sample sealed now): no part of
    /// any visit.
    pub withheld: &'a BTreeSet<i64>,
}

/// A pick stored on a visit, applying: a person's or a run's.
#[derive(Debug, Clone)]
struct Stored {
    id: i64,
    person: bool,
    stacks: Vec<i64>,
    why: Option<String>,
}

#[derive(Debug, Clone)]
struct Visit {
    day: Day,
    session_id: i64,
    candidates: Vec<Candidate>,
    stored: Option<Stored>,
}

#[derive(Debug, Clone)]
struct Subject {
    id: i64,
    code: String,
    visits: Vec<Visit>,
}

/// A scope's visits for one role, read.
pub struct Read {
    subjects: Vec<Subject>,
    datasets: HashMap<i64, Vec<String>>,
}

/// What one visit takes under one set of rules.
#[derive(Debug, Clone, Default)]
struct Taken {
    kind: Option<String>,
    stacks: Vec<i64>,
    /// `rules` or `person`.
    by: &'static str,
    chosen: Chosen,
}

/// Columns by scanner or by dataset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Columns {
    Scanner,
    Dataset,
}

impl Columns {
    pub fn parse(word: &str) -> Option<Columns> {
        match word {
            "scanner" => Some(Columns::Scanner),
            "dataset" => Some(Columns::Dataset),
            _ => None,
        }
    }

    fn other(self) -> &'static str {
        match self {
            Columns::Scanner => "Other scanners",
            Columns::Dataset => "Other datasets",
        }
    }
}

/// Read a scope's visits for one role: its subjects' stacks holding it,
/// grouped into candidates as a run groups them, by visit in order, and the
/// picks stored on them in the reading scope.
pub fn read(registry: &mut Registry, ask: &Ask<'_>) -> Result<Read, Error> {
    let store = registry.store();
    let labels = nils_session::labels_by_study(store, ask.scheme).map_err(|e| match e {
        nils_session::Error::Store(s) => Error::Store(s),
        nils_session::Error::Message(m) => Error::Store(StoreError::Message(m)),
    })?;
    let mut names = picking::carried(ask.model);
    names.extend(COLUMNS.iter().map(|n| n.to_string()));
    names.sort();
    names.dedup();
    let mut rows = picking::read_named(store, &names, None, Some(ask.subjects))?;
    rows.retain(|r| !ask.withheld.contains(&r.stack));
    let mine: Vec<&Row> = picking::of_role(ask.model, ask.role, &rows);
    let mut by_subject: BTreeMap<i64, BTreeMap<i64, (Day, Vec<&Row>)>> = BTreeMap::new();
    for r in &mine {
        let Some(l) = labels.get(&r.study) else {
            continue;
        };
        by_subject
            .entry(r.subject)
            .or_default()
            .entry(l.session_id)
            .or_insert_with(|| (l.first, Vec::new()))
            .1
            .push(r);
    }
    let codes = codes(store, ask.subjects)?;
    let stored = stored(store, ask)?;
    let mut subjects = Vec::with_capacity(by_subject.len());
    for (id, occasions) in by_subject {
        let mut visits: Vec<Visit> = occasions
            .into_iter()
            .map(|(session_id, (day, here))| Visit {
                day,
                session_id,
                candidates: picking::group_with(ask.model, &here, &names),
                stored: stored.get(&(id, day.to_string())).cloned(),
            })
            .collect();
        visits.sort_by_key(|v| v.day);
        subjects.push(Subject {
            id,
            code: codes.get(&id).cloned().unwrap_or_default(),
            visits,
        });
    }
    Ok(Read {
        subjects,
        datasets: ask.datasets.clone(),
    })
}

fn codes(store: &mut Store, ids: &BTreeSet<i64>) -> Result<HashMap<i64, String>, StoreError> {
    let ids: Vec<i64> = ids.iter().copied().collect();
    let mut out = HashMap::new();
    for chunk in ids.chunks(500) {
        let sql = format!(
            "SELECT id, code FROM {} WHERE id IN ({})",
            store.qualified("subject"),
            list(chunk)
        );
        for r in store.query(&sql, &[])? {
            out.insert(r.int(0)?, r.text(1)?.to_string());
        }
    }
    Ok(out)
}

fn list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The picks applying on the scope's visits for the role, in the reading
/// scope, by subject and day: a person's where one stands, else a run's.
fn stored(store: &mut Store, ask: &Ask<'_>) -> Result<HashMap<(i64, String), Stored>, StoreError> {
    let ids: Vec<i64> = ask.subjects.iter().copied().collect();
    let t = table("pick");
    let d = store.dialect();
    let day = d.text_of(t.column("session_day").expect("pick.session_day"));
    let mut out: HashMap<(i64, String), Stored> = HashMap::new();
    for chunk in ids.chunks(500) {
        let sql = format!(
            "SELECT id, subject_id, {day}, author_kind, why FROM {} p WHERE role = {} \
             AND model = {} AND scheme_digest = {} AND subject_id IN ({}) ORDER BY id",
            ask.picks.picks(store),
            d.param(1, Type::Text),
            d.param(2, Type::Text),
            d.param(3, Type::Text),
            list(chunk)
        );
        let rows = store.query(
            &sql,
            &[
                Param::from(ask.role),
                Param::from(ask.model.name.as_str()),
                Param::from(ask.scheme.digest()),
            ],
        )?;
        let mut found: Vec<((i64, String), Stored)> = Vec::new();
        for r in &rows {
            let day = r.text(2)?;
            found.push((
                (r.int(1)?, day[..day.len().min(10)].to_string()),
                Stored {
                    id: r.int(0)?,
                    person: r.text(3)? == "person",
                    stacks: Vec::new(),
                    why: r.opt_text(4)?.map(str::to_string),
                },
            ));
        }
        if found.is_empty() {
            continue;
        }
        let picks: Vec<i64> = found.iter().map(|(_, s)| s.id).collect();
        let mut stacks: HashMap<i64, Vec<i64>> = HashMap::new();
        for r in store.query(
            &format!(
                "SELECT pick_id, stack_id FROM {} WHERE pick_id IN ({}) ORDER BY stack_id",
                store.qualified("pick_stack"),
                list(&picks)
            ),
            &[],
        )? {
            stacks.entry(r.int(0)?).or_default().push(r.int(1)?);
        }
        for (key, mut s) in found {
            s.stacks = stacks.remove(&s.id).unwrap_or_default();
            // a person's pick over a run's
            match out.get(&key) {
                Some(have) if have.person && !s.person => {}
                _ => {
                    out.insert(key, s);
                }
            }
        }
    }
    Ok(out)
}

/// The populations of one registry, by pick and role, each with the marker
/// of the stacks and axes it was built from: kept by whoever serves that
/// registry, never shared between registries.
#[derive(Debug, Default)]
pub struct References(Mutex<HashMap<(String, String), (String, Reference)>>);

/// The registry's population for a role, which a pick within a kind is
/// scored against, kept in `cache` until the registry's stacks or axes
/// change (their counts, their ids and their values' lengths).
pub fn reference(
    store: &mut Store,
    model: &Model,
    role: &str,
    cache: &References,
) -> Result<Reference, Error> {
    let r = &store.query(
        &format!(
            // a sum is NUMERIC on Postgres, which the store reads only as
            // text: cast to BIGINT, as SQLite reads it too
            "SELECT (SELECT COUNT(*) FROM {f}), (SELECT CAST(COALESCE(SUM(stack_id), 0) AS BIGINT) FROM {f}), \
                    (SELECT COUNT(*) FROM {a}), \
                    (SELECT CAST(COALESCE(SUM(stack_id + LENGTH(value)), 0) AS BIGINT) FROM {a})",
            f = store.qualified("stack_fingerprint"),
            a = store.qualified("classification_axis"),
        ),
        &[],
    )?[0];
    let marker = format!("{}|{}|{}|{}", r.int(0)?, r.int(1)?, r.int(2)?, r.int(3)?);
    let key = (model.name.clone(), role.to_string());
    if let Ok(held) = cache.0.lock()
        && let Some((m, reference)) = held.get(&key)
        && *m == marker
    {
        return Ok(reference.clone());
    }
    let rows = picking::read_named(store, &model.reads(), None, None)?;
    let mine = picking::of_role(model, role, &rows);
    let reference = picking::build_reference(model, "registry", &mine);
    if let Ok(mut held) = cache.0.lock() {
        held.insert(key, (marker, reference.clone()));
    }
    Ok(reference)
}

impl Read {
    /// What each visit takes under `rules`: a person's pick where one
    /// stands, else the rules' choice; the scan within the kind when a
    /// reference is given. With `stored`, the saved side: what the picks
    /// stored say, as every other reader of the scope reads them (one place
    /// of truth, 2026-10-11), the rules' choice standing only beside it.
    fn taken(
        &self,
        rules: &Rules,
        model: &Model,
        role: &str,
        reference: Option<&Reference>,
        stored: bool,
    ) -> Vec<Vec<Taken>> {
        let chooser = Chooser::new(rules, model);
        self.subjects
            .iter()
            .map(|s| {
                let visits: Vec<Vec<Candidate>> =
                    s.visits.iter().map(|v| v.candidates.clone()).collect();
                let chosen = chooser.choose(role, &visits, reference);
                s.visits
                    .iter()
                    .zip(chosen)
                    .map(|(v, c)| match &v.stored {
                        Some(p) if p.person || stored => Taken {
                            kind: p.stacks.first().and_then(|first| {
                                v.candidates
                                    .iter()
                                    .find(|c| c.stacks.contains(first))
                                    .map(pick_rules::kind_of)
                            }),
                            stacks: p.stacks.clone(),
                            by: if p.person { "person" } else { "rules" },
                            chosen: c,
                        },
                        // nothing stored: no pick, as every reader says
                        None if stored => Taken {
                            kind: None,
                            stacks: Vec::new(),
                            by: "rules",
                            chosen: c,
                        },
                        _ => Taken {
                            kind: c.kind.clone(),
                            stacks: c
                                .winner
                                .as_ref()
                                .map(|w| w.stacks.clone())
                                .unwrap_or_default(),
                            by: "rules",
                            chosen: c,
                        },
                    })
                    .collect()
            })
            .collect()
    }

    /// A visit's columns and field: those of the scan taken, else of its
    /// first candidate. By scanner a visit is in one column; by dataset in
    /// one for every dataset whose folder holds a file of the scan, so a
    /// scan two datasets hold is in both (2026-10-12: it was in the first
    /// that read it alone).
    fn columns_of(&self, v: &Visit, t: &Taken, by: Columns) -> (Vec<String>, String) {
        let c = t
            .stacks
            .first()
            .and_then(|s| v.candidates.iter().find(|c| c.stacks.contains(s)))
            .or(v.candidates.first());
        let Some(c) = c else {
            return (vec![String::new()], String::new());
        };
        let field = match c.get("field_strength_normalized").trim().parse::<f64>() {
            Ok(f) => format!("{} T", trim_number(f)),
            Err(_) => String::new(),
        };
        let labels = match by {
            Columns::Scanner => {
                let maker = maker(c.get("manufacturer"));
                let name = c.get("manufacturer_model_name").trim();
                let scanner = match (maker.is_empty(), name.is_empty()) {
                    (true, true) => "Unknown scanner".to_string(),
                    (false, true) => maker,
                    (true, false) => name.to_string(),
                    (false, false) => format!("{maker} {name}"),
                };
                vec![with_field(&scanner, &field)]
            }
            Columns::Dataset => {
                let mut names: Vec<&str> = c
                    .stacks
                    .iter()
                    .filter_map(|s| self.datasets.get(s))
                    .flatten()
                    .map(String::as_str)
                    .collect();
                names.sort_unstable();
                names.dedup();
                if names.is_empty() {
                    names.push("No dataset");
                }
                names.iter().map(|n| with_field(n, &field)).collect()
            }
        };
        (labels, field)
    }
}

fn with_field(name: &str, field: &str) -> String {
    if field.is_empty() {
        name.to_string()
    } else {
        format!("{name} · {field}")
    }
}

fn trim_number(x: f64) -> String {
    let s = format!("{x:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A manufacturer as a person names it.
fn maker(raw: &str) -> String {
    let low = raw.trim().to_ascii_lowercase();
    for (needle, name) in [
        ("siemens", "Siemens"),
        ("philips", "Philips"),
        ("toshiba", "Canon"),
        ("canon", "Canon"),
        ("hitachi", "Hitachi"),
        ("general electric", "GE"),
        ("ge medical", "GE"),
        ("ge healthcare", "GE"),
    ] {
        if low.contains(needle) {
            return name.to_string();
        }
    }
    if low == "ge" {
        return "GE".to_string();
    }
    raw.trim().to_string()
}

/// The five numbers of section 5 for one side.
fn metrics(read: &Read, taken: &[Vec<Taken>]) -> Value {
    let (mut visits, mut visits_taken, mut subjects, mut subjects_taken) = (0, 0, 0, 0);
    let mut by_kind: BTreeMap<&str, i64> = BTreeMap::new();
    let (mut within, mut within_of, mut complete, mut complete_of) = (0, 0, 0, 0);
    for (s, t) in read.subjects.iter().zip(taken) {
        if s.visits.is_empty() {
            continue;
        }
        subjects += 1;
        visits += s.visits.len();
        let mine: Vec<&Taken> = t.iter().filter(|x| x.kind.is_some()).collect();
        visits_taken += mine.len();
        if !mine.is_empty() {
            subjects_taken += 1;
        }
        for x in &mine {
            if let Some(k) = &x.kind {
                *by_kind.entry(k.as_str()).or_insert(0) += 1;
            }
        }
        if mine.len() >= 2 {
            within_of += 1;
            let kinds: BTreeSet<Option<&String>> = mine.iter().map(|x| x.kind.as_ref()).collect();
            if kinds.len() == 1 {
                within += 1;
            }
        }
        if s.visits.len() >= 2 {
            complete_of += 1;
            if mine.len() == s.visits.len() {
                complete += 1;
            }
        }
    }
    let alike = by_kind
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(k, n)| json!({"kind": k, "visits": n}))
        .unwrap_or(json!({"kind": null, "visits": 0}));
    json!({
        "visits": visits, "visits_taken": visits_taken,
        "subjects": subjects, "subjects_taken": subjects_taken,
        "alike_data": alike,
        "alike_within": {"subjects": within, "of": within_of},
        "series_complete": {"subjects": complete, "of": complete_of},
    })
}

/// A subject's series of kinds: its taken kinds in order, a run of one
/// kind written once, a visit with nothing taken left out.
fn steps(t: &[Taken]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for x in t {
        if let Some(k) = &x.kind
            && out.last() != Some(k)
        {
            out.push(k.clone());
        }
    }
    out
}

/// One side of the map: the rules and what each visit takes under them.
pub struct Side {
    rules: Rules,
    taken: Vec<Vec<Taken>>,
}

/// What a map door asks: the saved rules, and a draft beside them.
pub struct Compare {
    pub saved: Side,
    pub draft: Option<Side>,
}

impl Compare {
    /// Each side evaluated; with a reference, down to the scan.
    pub fn new(
        read: &Read,
        model: &Model,
        role: &str,
        saved: Rules,
        draft: Option<Rules>,
        reference: Option<&Reference>,
    ) -> Compare {
        // the saved side is what the stored picks say; a draft is the
        // rules' choice, beside them
        let taken = read.taken(&saved, model, role, reference, true);
        let draft = draft.map(|rules| Side {
            taken: read.taken(&rules, model, role, reference, false),
            rules,
        });
        Compare {
            saved: Side {
                rules: saved,
                taken,
            },
            draft,
        }
    }

    /// The side shown: the draft where there is one.
    fn shown(&self) -> &Side {
        self.draft.as_ref().unwrap_or(&self.saved)
    }

    /// Whether a visit takes another kind or scan under the draft.
    fn changed(&self, s: usize, v: usize) -> bool {
        match &self.draft {
            None => false,
            Some(d) => {
                let (a, b) = (&self.saved.taken[s][v], &d.taken[s][v]);
                a.kind != b.kind || a.stacks != b.stacks
            }
        }
    }
}

/// One column of the map: its key, how many visits it shows, and how many
/// of those another column shows too.
#[derive(Debug, Clone, PartialEq)]
struct Column {
    key: String,
    visits: i64,
    shared: i64,
}

impl Column {
    fn to_json(&self) -> Value {
        json!({"key": self.key, "visits": self.visits, "shared": self.shared})
    }
}

/// Where the map puts every visit under the shown side.
#[derive(Debug, Clone, PartialEq)]
struct Laid {
    /// Each visit's columns, in the columns' order, and its field
    /// strength, by subject and visit.
    visits: Vec<Vec<(Vec<String>, String)>>,
    /// The named columns, then the rest as one.
    columns: Vec<Column>,
    /// The visits shown under more than one column.
    shared: i64,
}

/// The columns of every visit under the shown side, the named ones and the
/// rest as one. A visit is counted once in each column it is shown under,
/// so where a scan several datasets hold shows it under each, the columns'
/// visits add up to more than the visits, and `shared` says how many
/// visits that is; the rest counts each visit once, as one column.
fn columns(read: &Read, taken: &[Vec<Taken>], by: Columns) -> Laid {
    let raw: Vec<Vec<(Vec<String>, String)>> = read
        .subjects
        .iter()
        .zip(taken)
        .map(|(s, t)| {
            s.visits
                .iter()
                .zip(t)
                .map(|(v, x)| read.columns_of(v, x, by))
                .collect()
        })
        .collect();
    let mut counts: BTreeMap<&str, i64> = BTreeMap::new();
    for (labels, _) in raw.iter().flatten() {
        for label in labels {
            *counts.entry(label.as_str()).or_insert(0) += 1;
        }
    }
    let mut ordered: Vec<(&str, i64)> = counts.into_iter().collect();
    ordered.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    let named: Vec<&str> = ordered
        .iter()
        .take(NAMED_COLUMNS)
        .map(|(k, _)| *k)
        .collect();
    let mut columns: Vec<Column> = named
        .iter()
        .map(|k| k.to_string())
        .chain(std::iter::once(by.other().to_string()))
        .map(|key| Column {
            key,
            visits: 0,
            shared: 0,
        })
        .collect();
    let mut shared = 0;
    let visits: Vec<Vec<(Vec<String>, String)>> = raw
        .iter()
        .map(|vs| {
            vs.iter()
                .map(|(labels, field)| {
                    // a column past the named ones is the rest, once a visit
                    let mut at: Vec<usize> = labels
                        .iter()
                        .map(|l| named.iter().position(|n| n == l).unwrap_or(named.len()))
                        .collect();
                    at.sort_unstable();
                    at.dedup();
                    let several = at.len() > 1;
                    if several {
                        shared += 1;
                    }
                    for i in &at {
                        columns[*i].visits += 1;
                        if several {
                            columns[*i].shared += 1;
                        }
                    }
                    let keys = at.iter().map(|i| columns[*i].key.clone()).collect();
                    (keys, field.clone())
                })
                .collect()
        })
        .collect();
    if columns.last().is_some_and(|c| c.visits == 0) {
        columns.pop();
    }
    Laid {
        visits,
        columns,
        shared,
    }
}

/// The kinds of the role in the shown rules' order: every kind a visit
/// holds, with how many hold it and take it.
fn kinds(read: &Read, cmp: &Compare, model: &Model, role: &str) -> (Vec<String>, Value) {
    let shown = cmp.shown();
    let Some(r) = shown.rules.role(role) else {
        return (Vec::new(), json!([]));
    };
    let order = pick_rules::KindOrder::new(model, role, r);
    let mut with: BTreeMap<String, i64> = BTreeMap::new();
    let mut taken: BTreeMap<String, i64> = BTreeMap::new();
    for (s, t) in read.subjects.iter().zip(&shown.taken) {
        for (v, x) in s.visits.iter().zip(t) {
            let held: BTreeSet<String> = v.candidates.iter().map(pick_rules::kind_of).collect();
            for k in held {
                *with.entry(k).or_insert(0) += 1;
            }
            if let Some(k) = &x.kind {
                *taken.entry(k.clone()).or_insert(0) += 1;
                with.entry(k.clone()).or_insert(0);
            }
        }
    }
    let mut keys: Vec<String> = with.keys().cloned().collect();
    order.sort(&mut keys);
    let doc: Vec<Value> = keys
        .iter()
        .map(|k| {
            let used = !r.not_used.contains(k);
            let mut on = r.clone();
            on.not_used.clear();
            json!({
                "key": k,
                "visits_with": with[k],
                "visits_taken": taken.get(k).copied().unwrap_or(0),
                "allowed": pick_rules::kind_allowed(&on, k),
                "used": used,
                "new": order.is_new(k),
            })
        })
        .collect();
    (keys, Value::Array(doc))
}

/// `POST /api/picks/map`: the summaries of the scope for the role, under
/// the draft where one is given, and its effect against the saved rules.
pub fn map(
    read: &Read,
    cmp: &Compare,
    model: &Model,
    role: &str,
    by: Columns,
    version: i64,
) -> Value {
    let shown = cmp.shown();
    let (keys, kinds) = kinds(read, cmp, model, role);
    let laid = columns(read, &shown.taken, by);
    let index: HashMap<&str, usize> = laid
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| (c.key.as_str(), i))
        .collect();
    let mut cells: BTreeMap<Option<String>, Vec<i64>> = BTreeMap::new();
    let mut series: BTreeMap<Vec<String>, i64> = BTreeMap::new();
    let mut single = 0;
    let mut by_visit: BTreeMap<usize, (i64, BTreeMap<Option<String>, i64>)> = BTreeMap::new();
    for ((s, t), cols) in read.subjects.iter().zip(&shown.taken).zip(&laid.visits) {
        if s.visits.len() == 1 {
            single += 1;
        } else if s.visits.len() >= 2 {
            *series.entry(steps(t)).or_insert(0) += 1;
        }
        for (i, (x, (under, _))) in t.iter().zip(cols).enumerate() {
            let kind = x.kind.clone();
            let row = cells
                .entry(kind.clone())
                .or_insert_with(|| vec![0; laid.columns.len()]);
            // a visit is in each column it is shown under
            for col in under {
                if let Some(c) = index.get(col.as_str()) {
                    row[*c] += 1;
                }
            }
            let at = by_visit.entry((i + 1).min(5)).or_default();
            at.0 += 1;
            *at.1.entry(kind).or_insert(0) += 1;
        }
    }
    let mut matrix: Vec<Value> = keys
        .iter()
        .filter_map(|k| {
            cells
                .get(&Some(k.clone()))
                .map(|c| json!({"kind": k, "cells": c}))
        })
        .collect();
    if let Some(c) = cells.get(&None) {
        matrix.push(json!({"kind": null, "cells": c}));
    }
    let mut series: Vec<(Vec<String>, i64)> = series.into_iter().collect();
    series.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let rank = |k: &Option<String>| match k {
        Some(k) => keys.iter().position(|x| x == k).unwrap_or(usize::MAX - 1),
        None => usize::MAX,
    };
    let mut out = json!({
        "role": role,
        "rules_version": version,
        "kinds": kinds,
        "metrics": metrics(read, &shown.taken),
        "columns": laid.columns.iter().map(Column::to_json).collect::<Vec<_>>(),
        // the visits shown under more than one column: their scan is in
        // several datasets, and each column counts them (2026-10-12)
        "shared_visits": laid.shared,
        "matrix": matrix,
        "series": series
            .iter()
            .map(|(steps, n)| json!({"steps": steps, "subjects": n}))
            .collect::<Vec<_>>(),
        "single_visit_subjects": single,
        "by_visit": by_visit
            .into_iter()
            .map(|(visit, (n, kinds))| {
                let mut kinds: Vec<(Option<String>, i64)> = kinds.into_iter().collect();
                kinds.sort_by_key(|(k, _)| rank(k));
                json!({
                    "visit": visit, "visits": n,
                    "kinds": kinds
                        .into_iter()
                        .map(|(k, n)| json!({"kind": k, "visits": n}))
                        .collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>(),
    });
    if cmp.draft.is_some() {
        let (mut visits, mut subjects) = (0, 0);
        for (si, s) in read.subjects.iter().enumerate() {
            let n = (0..s.visits.len()).filter(|v| cmp.changed(si, *v)).count();
            visits += n;
            if n > 0 {
                subjects += 1;
            }
        }
        out["effect"] = json!({
            "visits_changed": visits, "subjects_changed": subjects,
            "before": metrics(read, &cmp.saved.taken),
        });
    }
    out
}

/// Which subjects a page of `POST /api/picks/map/subjects` is of.
#[derive(Debug, Clone, PartialEq)]
pub enum Group {
    /// Subjects whose kind changes between visits.
    Breaks,
    Series(Vec<String>),
    Cell {
        kind: Option<String>,
        column: String,
    },
}

impl Group {
    pub fn parse(doc: &Value) -> Result<Group, String> {
        match doc["by"].as_str() {
            Some("breaks") => Ok(Group::Breaks),
            Some("series") => Ok(Group::Series(
                doc["steps"]
                    .as_array()
                    .ok_or("a series group names its steps")?
                    .iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect(),
            )),
            Some("cell") => Ok(Group::Cell {
                kind: doc["kind"].as_str().map(str::to_string),
                column: doc["column"]
                    .as_str()
                    .ok_or("a cell group names its column")?
                    .to_string(),
            }),
            _ => Err("group.by: breaks, series or cell".to_string()),
        }
    }
}

/// `POST /api/picks/map/subjects`: one page of a group's subjects, each
/// with its visits as the shown side takes them. `order` is `changes`
/// (the visits the draft changes, then the kinds' breaks) or `visits`.
#[allow(clippy::too_many_arguments)]
pub fn subjects(
    read: &Read,
    cmp: &Compare,
    by: Columns,
    group: &Group,
    by_changes: bool,
    page: usize,
    per_page: usize,
) -> Value {
    let shown = cmp.shown();
    let labels = columns(read, &shown.taken, by).visits;
    let mut found: Vec<(usize, usize, usize)> = Vec::new();
    for (si, (s, t)) in read.subjects.iter().zip(&shown.taken).enumerate() {
        let kinds: BTreeSet<&String> = t.iter().filter_map(|x| x.kind.as_ref()).collect();
        let member = match group {
            Group::Breaks => kinds.len() >= 2,
            Group::Series(want) => s.visits.len() >= 2 && steps(t) == *want,
            Group::Cell { kind, column } => t
                .iter()
                .zip(&labels[si])
                .any(|(x, (at, _))| x.kind == *kind && at.iter().any(|c| c == column)),
        };
        if member {
            let changed = (0..s.visits.len()).filter(|v| cmp.changed(si, *v)).count();
            found.push((si, changed, kinds.len()));
        }
    }
    found.sort_by(|a, b| {
        let (sa, sb) = (&read.subjects[a.0], &read.subjects[b.0]);
        if by_changes {
            b.1.cmp(&a.1)
                .then_with(|| b.2.cmp(&a.2))
                .then_with(|| sa.code.cmp(&sb.code))
        } else {
            sb.visits
                .len()
                .cmp(&sa.visits.len())
                .then_with(|| sa.code.cmp(&sb.code))
        }
    });
    let total = found.len();
    let page_of: Vec<Value> = found
        .iter()
        .skip(page.saturating_mul(per_page))
        .take(per_page)
        .map(|(si, _, _)| {
            let s = &read.subjects[*si];
            let visits: Vec<Value> = s
                .visits
                .iter()
                .zip(&shown.taken[*si])
                .zip(&labels[*si])
                .enumerate()
                .map(|(vi, ((v, x), (at, field)))| {
                    let stored = v.stored.as_ref();
                    // no date leaves the door: a visit is its session's id
                    json!({
                        "session_id": v.session_id,
                        "visit": vi + 1,
                        // the first of the columns it is shown under, which
                        // are several where several datasets hold its scan
                        "column": at.first(),
                        "columns": at,
                        "field": field,
                        "kind": x.kind.clone(),
                        "stack": x.stacks.first(),
                        "stacks": x.stacks,
                        "by": x.by,
                        "changed": cmp.changed(*si, vi),
                        "pick": stored.map(|p| p.id),
                        "why": stored.and_then(|p| p.why.clone()),
                        "candidates": candidates(v, &x.chosen),
                    })
                })
                .collect();
            json!({"subject_id": s.id, "subject": s.code, "visits": visits})
        })
        .collect();
    json!({"total": total, "page": page, "per_page": per_page, "subjects": page_of})
}

/// A visit's candidates of the role, with their kinds and, where the rules
/// scored them within the kind taken, their scores.
fn candidates(v: &Visit, chosen: &Chosen) -> Vec<Value> {
    let scores: HashMap<&Vec<i64>, f64> = chosen.considered.iter().map(|(s, x)| (s, *x)).collect();
    v.candidates
        .iter()
        .map(|c| {
            json!({
                "stacks": c.stacks,
                "kind": pick_rules::kind_of(c),
                "score": scores.get(&c.stacks),
            })
        })
        .collect()
}

/// `POST /api/picks/map/strips`: every subject's kinds per visit,
/// compactly, by index into `kinds`; `-1` is no pick.
pub fn strips(read: &Read, cmp: &Compare, model: &Model, role: &str, by: Columns) -> Value {
    let shown = cmp.shown();
    let (keys, _) = kinds(read, cmp, model, role);
    let laid = columns(read, &shown.taken, by);
    let index: HashMap<&str, usize> = laid
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| (c.key.as_str(), i))
        .collect();
    let subjects: Vec<Value> = read
        .subjects
        .iter()
        .zip(&shown.taken)
        .zip(&laid.visits)
        .map(|((s, t), cols)| {
            let mut count: BTreeMap<usize, usize> = BTreeMap::new();
            for col in cols.iter().flat_map(|(at, _)| at) {
                if let Some(i) = index.get(col.as_str()) {
                    *count.entry(*i).or_insert(0) += 1;
                }
            }
            let column = count
                .iter()
                .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
                .map_or(0, |(i, _)| *i);
            let visits: Vec<i64> = t
                .iter()
                .map(|x| match &x.kind {
                    Some(k) => keys.iter().position(|y| y == k).map_or(-1, |i| i as i64),
                    _ => -1,
                })
                .collect();
            json!({"subject_id": s.id, "column": column, "visits": visits})
        })
        .collect();
    json!({
        "kinds": keys,
        "columns": laid.columns.iter().map(Column::to_json).collect::<Vec<_>>(),
        "shared_visits": laid.shared,
        "subjects": subjects,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_maker_and_a_field_read_as_a_person_names_them() {
        assert_eq!(maker("SIEMENS"), "Siemens");
        assert_eq!(maker("GE MEDICAL SYSTEMS"), "GE");
        assert_eq!(maker("Philips Medical Systems"), "Philips");
        assert_eq!(maker("Acme"), "Acme");
        assert_eq!(trim_number(3.0), "3");
        assert_eq!(trim_number(1.5), "1.5");
        assert_eq!(with_field("Siemens Skyra", "3 T"), "Siemens Skyra · 3 T");
    }

    #[test]
    fn a_series_writes_a_run_of_one_kind_once_and_leaves_out_nothing_taken() {
        let t = |k: Option<&str>, stacks: &[i64]| Taken {
            kind: k.map(str::to_string),
            stacks: stacks.to_vec(),
            by: "rules",
            chosen: Chosen::default(),
        };
        let got = steps(&[
            t(Some("2D SE"), &[1]),
            t(Some("2D SE"), &[2]),
            t(None, &[]),
            t(Some("3D MPRAGE"), &[3]),
        ]);
        assert_eq!(got, ["2D SE", "3D MPRAGE"]);
        assert_eq!(
            Group::parse(&json!({"by": "cell", "kind": null, "column": "x"})).unwrap(),
            Group::Cell {
                kind: None,
                column: "x".into()
            }
        );
        assert!(Group::parse(&json!({"by": "nothing"})).is_err());
    }

    /// Nima, 2026-10-12: a dataset is a folder that holds stacks, and a
    /// stack copied into two datasets is one stack with a path in each. By
    /// dataset, a visit whose scan two named datasets hold is in both
    /// columns and each counts it; the map says how many visits are shown
    /// under more than one column; and the rest counts a visit once,
    /// however many of the datasets past the named ones hold its scan.
    #[test]
    fn a_scan_several_datasets_hold_is_in_each_column_and_the_rest_counts_it_once() {
        let held: &[&[&str]] = &[
            &["a", "b"],
            &["a"],
            &["b"],
            &["c"],
            &["c"],
            &["d"],
            &["d"],
            &["e"],
            &["e"],
            &["f"],
            &["f"],
            &["g"],
            &["g"],
            &["h"],
            &["h"],
            &["x", "y"],
            &["a", "x"],
            &[],
        ];
        let mut datasets: HashMap<i64, Vec<String>> = HashMap::new();
        let mut subjects = Vec::new();
        let mut taken = Vec::new();
        for (i, names) in held.iter().enumerate() {
            let stack = i as i64 + 1;
            if !names.is_empty() {
                datasets.insert(stack, names.iter().map(|n| n.to_string()).collect());
            }
            subjects.push(Subject {
                id: stack,
                code: format!("S{stack}"),
                visits: vec![Visit {
                    day: Day::new(2024, 1, 1).unwrap(),
                    session_id: stack,
                    candidates: vec![Candidate {
                        stacks: vec![stack],
                        ..Candidate::default()
                    }],
                    stored: None,
                }],
            });
            taken.push(vec![Taken {
                kind: Some("3D MPRAGE".into()),
                stacks: vec![stack],
                by: "rules",
                chosen: Chosen::default(),
            }]);
        }
        let read = Read { subjects, datasets };
        let laid = columns(&read, &taken, Columns::Dataset);
        let at = |stack: usize| laid.visits[stack - 1][0].0.clone();
        assert_eq!(at(1), ["a", "b"]);
        assert_eq!(at(2), ["a"]);
        assert_eq!(at(16), ["Other datasets"], "x and y are the rest, once");
        assert_eq!(at(17), ["a", "Other datasets"]);
        assert_eq!(at(18), ["Other datasets"], "no dataset is past the named");
        let column = |key: &str, visits: i64, shared: i64| Column {
            key: key.to_string(),
            visits,
            shared,
        };
        assert_eq!(
            laid.columns,
            [
                column("a", 3, 2),
                column("b", 2, 1),
                column("c", 2, 0),
                column("d", 2, 0),
                column("e", 2, 0),
                column("f", 2, 0),
                column("g", 2, 0),
                column("h", 2, 0),
                column("Other datasets", 3, 1),
            ]
        );
        assert_eq!(laid.shared, 2);
        // the columns add up to the visits and the shared ones once more
        let shown: i64 = laid.columns.iter().map(|c| c.visits).sum();
        assert_eq!(shown, held.len() as i64 + laid.shared);
        // by scanner a visit is in one column
        let laid = columns(&read, &taken, Columns::Scanner);
        assert_eq!(laid.shared, 0);
        assert_eq!(laid.columns, [column("Unknown scanner", 18, 0)]);
    }
}
