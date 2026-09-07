// SPDX-License-Identifier: AGPL-3.0-only

//! The compiler (§11): one statement per ask, one SQL text per backend,
//! sets in topological order as CTEs, each set a layered subselect in the
//! order of rule 5. The eleven hooks and the four closures live in
//! [`Sql`], the only place the two dialects differ; everything above it is
//! the same text.
//!
//! The base relation of every grain; `from` (a set, a role, a handle, an
//! uploaded list); `of`; `algebra`; `group` with its aggregates; `near`
//! with its five policies, precision aware; `attach`; `has`; `bind` with
//! the functions, the sequences, `change`, `share` and the derived fields
//! including the level signature; `where`; `pick` with its ties; and the
//! answer with its columns, order, keyset paging and limit. Every set's
//! CTE projects the same spine and its frame records everything it
//! exposes by name, so a reader, a partner or a group reads it by column.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use nils_registry::dialect::{Dialect, qualified};
use nils_registry::schema::Type;
use nils_registry::store::Param;
use serde_json::Value;

use crate::ast::{AlgOp, Arg, Ask, Clause, Dir, Grain, IntSpec, ParamType, Set, Src};
use crate::validate::{ColumnRef, Names, Validated};

#[derive(Debug, Clone, PartialEq)]
pub struct CompileError {
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

impl std::error::Error for CompileError {}

type R<T> = Result<T, CompileError>;

fn err(path: impl Into<String>, message: impl Into<String>) -> CompileError {
    CompileError {
        path: path.into(),
        message: message.into(),
    }
}

/// What the compiler needs beside the document.
pub struct Context<'a> {
    pub names: &'a dyn Names,
    pub dialect: Dialect,
    /// The Postgres schema the tables live in; none on SQLite.
    pub schema: Option<String>,
    /// The document's scheme: its window, and its digest for the labels.
    pub window_days: i64,
    pub scheme_digest: String,
    /// Keyset paging of the answer: rows after this key, at most this many.
    pub after: Option<i64>,
    pub limit: Option<u64>,
}

/// One statement, its parameters in order, and the answer's columns.
#[derive(Debug, Clone, PartialEq)]
pub struct Compiled {
    pub sql: String,
    pub params: Vec<Param>,
    /// The answer's column names, `_key` and `_subject` first.
    pub columns: Vec<String>,
    /// Which answer columns are subject codes, digested before hashing.
    pub code_columns: Vec<usize>,
}

/// The dialect layer: the hooks (H1 to H11) and the closures (§11.2, §11.3).
#[derive(Debug, Clone, Copy)]
pub struct Sql {
    pub dialect: Dialect,
}

impl Sql {
    fn is_pg(self) -> bool {
        matches!(self.dialect, Dialect::Postgres)
    }

    /// H1.
    pub fn days_between(self, a: &str, b: &str) -> String {
        if self.is_pg() {
            format!("({a} - {b})")
        } else {
            format!("CAST(julianday({a}) - julianday({b}) AS INTEGER)")
        }
    }

    /// H2, in days: every unit is days (§5.1).
    pub fn shift_days(self, d: &str, days: &str) -> String {
        if self.is_pg() {
            format!("({d} + ({days})::int)")
        } else {
            format!("date({d}, '+' || CAST({days} AS TEXT) || ' days')")
        }
    }

    /// H3: birthday exact whole years. Each date is read twice, so a
    /// parameter comes as two placeholders.
    pub fn age_at(self, birth: (&str, &str), day: (&str, &str)) -> String {
        let (birth_y, birth_md) = birth;
        let (day_y, day_md) = day;
        if self.is_pg() {
            format!(
                "(EXTRACT(YEAR FROM {day_y})::int - EXTRACT(YEAR FROM {birth_y})::int \
                 - CASE WHEN to_char({day_md}, 'MMDD') < to_char({birth_md}, 'MMDD') THEN 1 ELSE 0 END)"
            )
        } else {
            format!(
                "(CAST(strftime('%Y', {day_y}) AS INTEGER) - CAST(strftime('%Y', {birth_y}) AS INTEGER) \
                 - CASE WHEN strftime('%m%d', {day_md}) < strftime('%m%d', {birth_md}) THEN 1 ELSE 0 END)"
            )
        }
    }

    /// H4, a rounded key: an integer at a literal scale, never `10^n`.
    pub fn rounded_key(self, x: &str, places: u32) -> String {
        let scale = 10i64.pow(places);
        if self.is_pg() {
            format!("CAST(round(({x} * {scale}.0)::numeric) AS BIGINT)")
        } else {
            format!("CAST(round({x} * {scale}.0) AS INTEGER)")
        }
    }

    /// H4, a rounded value for display.
    pub fn rounded(self, x: &str, places: u32) -> String {
        if self.is_pg() {
            format!("round(({x})::numeric, {places})::double precision")
        } else {
            format!("round({x}, {places})")
        }
    }

    /// H5, a sorted list.
    pub fn sorted_list(self, x: &str) -> String {
        if self.is_pg() {
            format!("string_agg(({x})::text, ',' ORDER BY ({x})::text)")
        } else {
            format!("group_concat({x}, ',' ORDER BY {x})")
        }
    }

    /// H8, the projection casts.
    pub fn as_bigint(self, x: &str) -> String {
        if self.is_pg() {
            format!("CAST({x} AS BIGINT)")
        } else {
            format!("CAST({x} AS INTEGER)")
        }
    }

    pub fn as_double(self, x: &str) -> String {
        if self.is_pg() {
            format!("CAST({x} AS DOUBLE PRECISION)")
        } else {
            format!("CAST({x} AS REAL)")
        }
    }

    /// H10, `bucket`: text, truncated to the unit.
    pub fn bucket(self, d: &str, unit: &str) -> R<String> {
        let (sq, pg) = match unit {
            "year" => ("%Y", "YYYY"),
            "month" => ("%Y-%m", "YYYY-MM"),
            "day" => ("%Y-%m-%d", "YYYY-MM-DD"),
            other => {
                return Err(err(
                    "bucket",
                    format!("{other} is not a bucket unit; those are year, month, day"),
                ));
            }
        };
        Ok(if self.is_pg() {
            format!("to_char({d}, '{pg}')")
        } else {
            format!("strftime('{sq}', {d})")
        })
    }

    /// H10, `part`: an integer.
    pub fn part(self, d: &str, unit: &str) -> R<String> {
        Ok(if self.is_pg() {
            let field = match unit {
                "year" => "YEAR",
                "month" => "MONTH",
                "day" => "DAY",
                "dow" => "DOW",
                other => {
                    return Err(err(
                        "part",
                        format!("{other} is not a part unit; those are year, month, day, dow"),
                    ));
                }
            };
            format!("EXTRACT({field} FROM {d})::int")
        } else {
            let fmt = match unit {
                "year" => "%Y",
                "month" => "%m",
                "day" => "%d",
                "dow" => "%w",
                other => {
                    return Err(err(
                        "part",
                        format!("{other} is not a part unit; those are year, month, day, dow"),
                    ));
                }
            };
            format!("CAST(strftime('{fmt}', {d}) AS INTEGER)")
        })
    }

    /// H11.
    pub fn least(self, a: &str, b: &str) -> String {
        if self.is_pg() {
            format!("LEAST({a}, {b})")
        } else {
            format!("MIN({a}, {b})")
        }
    }

    pub fn greatest(self, a: &str, b: &str) -> String {
        if self.is_pg() {
            format!("GREATEST({a}, {b})")
        } else {
            format!("MAX({a}, {b})")
        }
    }

    /// The closure on `->>`: text on both.
    pub fn json_text(self, x: &str, key: &str) -> String {
        if self.is_pg() {
            format!("(({x})::jsonb ->> '{key}')")
        } else {
            format!("CAST(({x} ->> '{key}') AS TEXT)")
        }
    }

    /// The closure on null order: NULLS LAST on both.
    pub fn order_term(self, x: &str, dir: Dir) -> String {
        let d = match dir {
            Dir::Asc => "ASC",
            Dir::Desc => "DESC",
        };
        if self.is_pg() {
            format!("{x} {d} NULLS LAST")
        } else {
            format!("CASE WHEN {x} IS NULL THEN 1 ELSE 0 END, {x} {d}")
        }
    }

    /// The last day of the interval a coarse date names (§5.3); a day is
    /// its own last day.
    pub fn interval_end(self, d: &str, prec: &str) -> String {
        if self.is_pg() {
            format!(
                "CASE {prec} WHEN 'year' THEN ({d} + INTERVAL '1 year' - INTERVAL '1 day')::date \
                 WHEN 'month' THEN ({d} + INTERVAL '1 month' - INTERVAL '1 day')::date ELSE {d} END"
            )
        } else {
            format!(
                "CASE {prec} WHEN 'year' THEN date({d}, '+1 year', '-1 day') \
                 WHEN 'month' THEN date({d}, '+1 month', '-1 day') ELSE {d} END"
            )
        }
    }

    /// A LIKE that escapes its pattern's own wildcards.
    pub fn like(self, x: &str, pattern_placeholder: &str) -> String {
        format!("{x} LIKE {pattern_placeholder} ESCAPE '\\'")
    }
}

/// Escape a literal for a LIKE pattern.
pub fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------- the frame

/// One expression with what it is, so a comparison can tell a coarse date.
#[derive(Debug, Clone)]
struct Term {
    sql: String,
    /// The precision column beside a date, when the date may be coarse.
    prec: Option<String>,
    /// The case folded companion, when the field has one.
    ci: Option<String>,
    /// The parameter the term binds, when it is one placeholder, so that a
    /// second use binds it again instead of naming a placeholder twice.
    param: Option<(Param, Type)>,
}

impl Term {
    fn plain(sql: String) -> Term {
        Term {
            sql,
            prec: None,
            ci: None,
            param: None,
        }
    }
}

/// What a set's CTE projects, by name, and how a reader reaches it.
#[derive(Debug, Clone, Default)]
struct Frame {
    /// Everything the CTE exposes: a name to a term whose `sql` is the
    /// bare column name in the CTE.
    terms: Vec<(String, Term)>,
    /// Bindings by name, as their column, the subset a union keeps.
    bindings: Vec<(String, String)>,
    /// A group's by paths, as their columns.
    group_by: Vec<(String, String)>,
    has_subj: bool,
    has_day: bool,
    has_prec: bool,
    of: Option<String>,
    picked: bool,
}

fn column_name(binding: &str) -> String {
    format!("b_{}", binding.replace('.', "__"))
}

fn field_col(path: &str) -> String {
    format!("f_{}", path.replace('.', "__"))
}

/// The base relation of a grain: its FROM clause, its key, and what it
/// carries. The aliases are fixed so a field's column can be named.
struct Base {
    from: String,
    key: String,
    subj: Option<String>,
    day: Option<String>,
    prec: Option<String>,
    session_k: Option<String>,
    study_k: Option<String>,
    series_k: Option<String>,
    stack_k: Option<String>,
    standing: Vec<String>,
}

fn alias_of(table: &str) -> Option<&'static str> {
    Some(match table {
        "cohort" => "c",
        "subject" => "su",
        "session_cache" => "sc",
        "session_label" => "sl",
        "study" => "sy",
        "series" => "se",
        "series_mr" => "smr",
        "series_ct" => "sct",
        "series_pet" => "spt",
        "stack" => "st",
        "stack_fingerprint" => "f",
        "instance" => "i",
        "event" => "e",
        "observation_type" => "ot",
        _ => return None,
    })
}

const SPINE: &[&str] = &[
    "k",
    "subj",
    "day",
    "prec",
    "session_k",
    "study_k",
    "series_k",
    "stack_k",
];

struct Builder<'a> {
    ctx: &'a Context<'a>,
    ask: &'a Ask,
    sql: Sql,
    params: Vec<Param>,
    ctes: Vec<String>,
    frames: BTreeMap<String, Frame>,
    reads: HashMap<String, usize>,
    /// The grain of the set being built.
    current: Option<Grain>,
    /// The field paths other sets read through an aggregate, a group or a
    /// partner over each set, which that set must project.
    external: HashMap<String, BTreeSet<String>>,
    answer_columns: Vec<String>,
    code_columns: Vec<usize>,
}

const AGGREGATES: &[&str] = &["count", "distinct", "min", "max", "sum", "avg", "list"];
const PHYSICS: &[&str] = &[
    "magnetic_field_strength",
    "field_strength_tesla",
    "repetition_time",
    "echo_time",
    "inversion_time",
    "flip_angle",
    "echo_train_length",
    "slice_thickness",
    "spacing_between_slices",
    "pixel_spacing_row",
    "pixel_spacing_col",
    "number_of_averages",
];

fn cte_name(set: &str) -> String {
    format!("s_{set}")
}

fn window_days(w: &crate::ast::WindowSpec, at: &str) -> R<(Option<i64>, Option<i64>)> {
    match w {
        crate::ast::WindowSpec::Literal(win) => Ok(win.days()),
        crate::ast::WindowSpec::Param(_) => Err(err(
            at,
            "a window parameter desugars into the document; desugar first",
        )),
    }
}

/// The field paths a clause reads.
fn paths_in(c: &Clause, out: &mut BTreeSet<String>) {
    let mut all = Vec::new();
    c.walk(&mut all);
    for cl in all {
        if cl.op == "field"
            && let Some(p) = cl.ref_name()
        {
            out.insert(p.to_string());
        }
    }
}

/// The derived refs a clause reads, with their options.
fn derived_in(c: &Clause, out: &mut Vec<(String, BTreeMap<String, Value>)>) {
    let mut all = Vec::new();
    c.walk(&mut all);
    for cl in all {
        if cl.op == "derived"
            && let Some(name) = cl.ref_name()
        {
            out.push((name.to_string(), cl.opts.clone()));
        }
    }
}

/// Every clause of a set, in the order they are read.
fn clauses_of(set: &Set) -> Vec<&Clause> {
    let mut out: Vec<&Clause> = Vec::new();
    for n in &set.near {
        out.extend(n.order.iter().map(|o| &o.0));
    }
    for (_, c) in &set.bind.0 {
        out.push(c);
    }
    out.extend(set.where_.iter());
    if let Some(pk) = &set.pick {
        out.extend(pk.by.iter().map(|o| &o.0));
    }
    out
}

/// For every set, the field paths other sets read through an aggregate
/// `{set: it}`, a group over it, or a partner relation naming it, to a
/// fixed point, since a partner's partner is read through two names.
fn external_paths(ask: &Ask) -> HashMap<String, BTreeSet<String>> {
    fn collect(c: &Clause, out: &mut HashMap<String, BTreeSet<String>>) {
        let mut all = Vec::new();
        c.walk(&mut all);
        for cl in all {
            if AGGREGATES.contains(&cl.op.as_str())
                && let Some(Value::String(target)) = cl.opts.get("set")
            {
                let mut paths = BTreeSet::new();
                for a in &cl.args {
                    if let Arg::Clause(inner) = a {
                        paths_in(inner, &mut paths);
                    }
                }
                out.entry(target.clone()).or_default().extend(paths);
            }
        }
    }
    let mut out: HashMap<String, BTreeSet<String>> = HashMap::new();
    for set in ask.sets.values() {
        for c in clauses_of(set) {
            collect(c, &mut out);
        }
        if let Some(g) = &set.group {
            let mut paths = BTreeSet::new();
            for c in &g.by {
                paths_in(c, &mut paths);
            }
            out.entry(g.of.clone()).or_default().extend(paths);
        }
    }
    for c in &ask.out.columns {
        collect(c, &mut out);
    }
    // partner paths, to a fixed point
    loop {
        let mut changed = false;
        for (name, set) in &ask.sets {
            let mut own: BTreeSet<String> = BTreeSet::new();
            for c in clauses_of(set) {
                paths_in(c, &mut own);
            }
            if *name == ask.out.set {
                for c in &ask.out.columns {
                    paths_in(c, &mut own);
                }
                for o in &ask.out.order {
                    paths_in(&o.0, &mut own);
                }
            }
            if let Some(ext) = out.get(name) {
                own.extend(ext.iter().cloned());
            }
            let partners: Vec<(&str, &str)> = set
                .near
                .iter()
                .map(|n| (n.as_.as_str(), n.set.as_str()))
                .chain(set.attach.iter().map(|a| (a.as_.as_str(), a.set.as_str())))
                .collect();
            for p in &own {
                for (as_, target) in &partners {
                    if let Some(rest) = p.strip_prefix(as_).and_then(|r| r.strip_prefix('.'))
                        && !matches!(
                            rest,
                            "date" | "precision" | "offset_days" | "tied" | "candidates"
                        )
                    {
                        let entry = out.entry((*target).to_string()).or_default();
                        if entry.insert(rest.to_string()) {
                            changed = true;
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    out
}

impl<'a> Builder<'a> {
    fn q(&self, table: &str) -> String {
        qualified(self.ctx.schema.as_deref(), table)
    }

    /// Bind one parameter and return its placeholder, numbered on both
    /// backends: the layers wrap each other, so an outer expression's
    /// placeholder sits before an inner one in the text while it is bound
    /// after it. On Postgres an integer or a double is cast explicitly:
    /// the driver sends an int8 and a placeholder beside an int4
    /// expression would infer int4.
    fn p(&mut self, v: Param, ty: Type) -> String {
        self.params.push(v);
        let n = self.params.len();
        match (self.ctx.dialect, ty) {
            (Dialect::Sqlite, _) => format!("?{n}"),
            (Dialect::Postgres, Type::Int) => format!("${n}::bigint"),
            (Dialect::Postgres, Type::Double) => format!("${n}::double precision"),
            (Dialect::Postgres, Type::Text) => format!("${n}::text"),
            (d, t) => d.param(n, t),
        }
    }

    /// A term used once more: a parameter is bound again, anything else is
    /// its text.
    fn again(&mut self, t: &Term) -> String {
        match &t.param {
            Some((v, ty)) => self.p(v.clone(), *ty),
            None => t.sql.clone(),
        }
    }

    fn base(&mut self, grain: Grain, path: &str) -> R<Base> {
        let q = |b: &Builder, t: &str| b.q(t);
        Ok(match grain {
            Grain::Cohort => Base {
                from: format!("{} c", q(self, "cohort")),
                key: "c.id".into(),
                subj: None,
                day: None,
                prec: None,
                session_k: None,
                study_k: None,
                series_k: None,
                stack_k: None,
                standing: Vec::new(),
            },
            Grain::Subject => Base {
                from: format!("{} su", q(self, "subject")),
                key: "su.id".into(),
                subj: Some("su.id".into()),
                day: None,
                prec: None,
                session_k: None,
                study_k: None,
                series_k: None,
                stack_k: None,
                standing: Vec::new(),
            },
            Grain::Session => {
                let digest = self.p(Param::from(self.ctx.scheme_digest.as_str()), Type::Text);
                let window = self.p(Param::Int(self.ctx.window_days), Type::Int);
                Base {
                    from: format!(
                        "{} sc JOIN {} su ON su.id = sc.subject_id \
                         LEFT JOIN {} sl ON sl.session_id = sc.id AND sl.scheme_digest = {digest}",
                        q(self, "session_cache"),
                        q(self, "subject"),
                        q(self, "session_label")
                    ),
                    key: "sc.id".into(),
                    subj: Some("su.id".into()),
                    day: Some("sc.first".into()),
                    prec: None,
                    session_k: Some("sc.id".into()),
                    study_k: None,
                    series_k: None,
                    stack_k: None,
                    standing: vec![format!("sc.window_days = {window}")],
                }
            }
            Grain::Stack => {
                let window = self.p(Param::Int(self.ctx.window_days), Type::Int);
                let digest = self.p(Param::from(self.ctx.scheme_digest.as_str()), Type::Text);
                Base {
                    from: format!(
                        "{} st JOIN {} se ON se.id = st.series_id JOIN {} sy ON sy.id = se.study_id \
                         JOIN {} su ON su.id = sy.subject_id JOIN {} f ON f.stack_id = st.id \
                         LEFT JOIN {} smr ON smr.series_id = se.id \
                         LEFT JOIN {} scs ON scs.study_id = sy.id AND scs.window_days = {window} \
                         LEFT JOIN {} sc ON sc.id = scs.session_id \
                         LEFT JOIN {} sl ON sl.session_id = sc.id AND sl.scheme_digest = {digest}",
                        q(self, "stack"),
                        q(self, "series"),
                        q(self, "study"),
                        q(self, "subject"),
                        q(self, "stack_fingerprint"),
                        q(self, "series_mr"),
                        q(self, "session_cache_study"),
                        q(self, "session_cache"),
                        q(self, "session_label")
                    ),
                    key: "st.id".into(),
                    subj: Some("su.id".into()),
                    day: Some("COALESCE(sy.date_filled, sy.study_date)".into()),
                    prec: None,
                    session_k: Some("scs.session_id".into()),
                    study_k: Some("sy.id".into()),
                    series_k: Some("se.id".into()),
                    stack_k: Some("st.id".into()),
                    standing: Vec::new(),
                }
            }
            Grain::Instance => {
                let window = self.p(Param::Int(self.ctx.window_days), Type::Int);
                let digest = self.p(Param::from(self.ctx.scheme_digest.as_str()), Type::Text);
                Base {
                    from: format!(
                        "{} i JOIN {} se ON se.id = i.series_id JOIN {} sy ON sy.id = se.study_id \
                         JOIN {} su ON su.id = sy.subject_id \
                         LEFT JOIN {} scs ON scs.study_id = sy.id AND scs.window_days = {window} \
                         LEFT JOIN {} sc ON sc.id = scs.session_id \
                         LEFT JOIN {} sl ON sl.session_id = sc.id AND sl.scheme_digest = {digest}",
                        q(self, "instance"),
                        q(self, "series"),
                        q(self, "study"),
                        q(self, "subject"),
                        q(self, "session_cache_study"),
                        q(self, "session_cache"),
                        q(self, "session_label")
                    ),
                    key: "i.id".into(),
                    subj: Some("su.id".into()),
                    day: Some("COALESCE(sy.date_filled, sy.study_date)".into()),
                    prec: None,
                    session_k: Some("scs.session_id".into()),
                    study_k: Some("sy.id".into()),
                    series_k: Some("se.id".into()),
                    stack_k: Some("i.stack_id".into()),
                    standing: Vec::new(),
                }
            }
            Grain::Event => Base {
                from: format!(
                    "{} e JOIN {} ot ON ot.id = e.observation_type_id JOIN {} su ON su.id = e.subject_id",
                    q(self, "event"),
                    q(self, "observation_type"),
                    q(self, "subject")
                ),
                key: "e.id".into(),
                subj: Some("su.id".into()),
                day: Some("e.event_date".into()),
                prec: Some("COALESCE(e.event_date_precision, 'day')".into()),
                session_k: None,
                study_k: None,
                series_k: None,
                stack_k: None,
                standing: vec!["e.superseded_by IS NULL".into()],
            },
            Grain::Group | Grain::Pair => {
                return Err(err(path, format!("{grain} has no base relation")));
            }
        })
    }

    /// The column of a field of a level in the base relation.
    fn base_column(&self, level: &str, path: &str) -> Option<Term> {
        let c: ColumnRef = self.ctx.names.column(level, path)?;
        if c.table == "study" && c.column == "day" {
            return Some(Term::plain(
                "COALESCE(sy.date_filled, sy.study_date)".into(),
            ));
        }
        let alias = alias_of(&c.table)?;
        let prec = (c.table == "event" && c.column == "event_date")
            .then(|| "COALESCE(e.event_date_precision, 'day')".to_string());
        Some(Term {
            sql: format!("{alias}.{}", c.column),
            prec,
            ci: c.ci.map(|ci| format!("{alias}.{ci}")),
            param: None,
        })
    }

    /// Whether a level's fields are reachable from a grain's base.
    fn level_reachable(grain: Grain, level: &str) -> bool {
        let levels: &[&str] = match grain {
            Grain::Cohort => &["cohort"],
            Grain::Subject => &["subject"],
            Grain::Session => &["session", "subject"],
            Grain::Stack => &["stack", "session", "series", "study", "subject"],
            Grain::Instance => &["instance", "stack", "session", "series", "study", "subject"],
            Grain::Event => &["event", "subject"],
            Grain::Group | Grain::Pair => &[],
        };
        levels.contains(&level)
    }

    /// A field path in a set's own base, for the first layer.
    fn field_term(&self, set: &Set, frame: &Frame, path: &str) -> Option<Term> {
        let grain = set.grain;
        let (first, rest) = match path.split_once('.') {
            Some((f, r)) => (f, Some(r)),
            None => (path, None),
        };
        if let (Some(rest), true) = (rest, Self::level_reachable(grain, first)) {
            if first == "cohort" && grain == Grain::Subject {
                return (rest == "id" && frame.of.is_some()).then(|| Term::plain("a.k".into()));
            }
            return self.base_column(first, rest);
        }
        if let Some(of) = &frame.of
            && of == first
            && let Some(rest) = rest
        {
            let ancestor = self.ask.sets.get(of)?;
            if ancestor.grain == Grain::Cohort {
                return (rest == "id").then(|| Term::plain("a.k".into()));
            }
            return self.base_column(ancestor.grain.name(), rest);
        }
        self.base_column(grain.name(), path)
    }

    /// The link from a child set's CTE to a parent grain's key.
    fn link(child: &Frame, parent: Grain, child_alias: &str, path: &str) -> R<String> {
        let col = match parent {
            Grain::Subject => "subj",
            Grain::Session => "session_k",
            Grain::Stack => "stack_k",
            Grain::Cohort => {
                return Err(err(
                    path,
                    "a cohort counts its subjects through the membership edge; count a subject set with of",
                ));
            }
            Grain::Instance | Grain::Event | Grain::Group | Grain::Pair => {
                return Err(err(path, format!("{parent} has no descendants to count")));
            }
        };
        if parent == Grain::Subject && !child.has_subj {
            return Err(err(path, "the counted set carries no subject"));
        }
        Ok(format!("{child_alias}.{col}"))
    }

    /// The inputs a derived field reads, to project them.
    fn derived_inputs(&self, name: &str, opts: &BTreeMap<String, Value>) -> Vec<String> {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        match name {
            "acquisition_type" => s(&["acquisition_type_filled", "mr_acquisition_type"]),
            "field_strength" => s(&["field_strength_tesla"]),
            "study_day" => Vec::new(),
            "voxel" => {
                let third = opts
                    .get("third")
                    .and_then(Value::as_str)
                    .unwrap_or("slice_thickness");
                vec![
                    "pixel_spacing_row".into(),
                    "pixel_spacing_col".into(),
                    third.to_string(),
                ]
            }
            "voxel_min" | "voxel_max" | "resolution" => {
                s(&["pixel_spacing_row", "pixel_spacing_col", "slice_thickness"])
            }
            "signature" => {
                let level = opts.get("level").and_then(Value::as_str).unwrap_or("loose");
                let mut out = s(&[
                    "acquisition_type_filled",
                    "mr_acquisition_type",
                    "orientation",
                ]);
                if let Some(spec) = self.ctx.names.level_spec(level) {
                    for m in spec.exact.iter().chain(spec.rounded.iter().map(|(k, _)| k)) {
                        if PHYSICS.contains(&m.as_str()) {
                            out.push(m.clone());
                        }
                    }
                }
                out
            }
            _ => Vec::new(),
        }
    }

    /// The field paths a set must project in its first layer.
    fn wanted_paths(&self, name: &str, set: &Set) -> BTreeSet<String> {
        let mut paths = BTreeSet::new();
        let mut derived: Vec<(String, BTreeMap<String, Value>)> = Vec::new();
        for c in clauses_of(set) {
            paths_in(c, &mut paths);
            derived_in(c, &mut derived);
        }
        if name == self.ask.out.set {
            for c in &self.ask.out.columns {
                paths_in(c, &mut paths);
                derived_in(c, &mut derived);
            }
            for o in &self.ask.out.order {
                paths_in(&o.0, &mut paths);
                derived_in(&o.0, &mut derived);
            }
        }
        if let Some(ext) = self.external.get(name) {
            paths.extend(ext.iter().cloned());
        }
        for h in &set.has {
            if let Some(on) = &h.on {
                paths.insert(on.clone());
            }
        }
        for n in &set.near {
            if let Some(on) = &n.on {
                paths.insert(on.clone());
            }
        }
        for (d, opts) in derived {
            paths.extend(self.derived_inputs(&d, &opts));
        }
        paths
    }

    fn push_cte(&mut self, name: &str, body: &str) {
        let reads = self.reads.get(name).copied().unwrap_or(0);
        let materialized = if reads > 1 { " MATERIALIZED" } else { "" };
        self.ctes
            .push(format!("{} AS{materialized} ({body})", cte_name(name)));
    }

    fn int_spec(&mut self, spec: &IntSpec, path: &str) -> R<String> {
        match spec {
            IntSpec::Literal(n) => Ok(n.to_string()),
            IntSpec::Param(c) => {
                let name = c
                    .ref_name()
                    .ok_or_else(|| err(path, "a param ref names a parameter"))?;
                let decl = self
                    .ask
                    .params
                    .get(name)
                    .ok_or_else(|| err(path, format!("no parameter {name}")))?;
                let v = decl
                    .value
                    .as_ref()
                    .ok_or_else(|| err(path, format!("parameter {name} has no value")))?;
                let n = v
                    .as_i64()
                    .ok_or_else(|| err(path, format!("parameter {name} is not an integer")))?;
                Ok(self.p(Param::Int(n), Type::Int))
            }
        }
    }

    /// The literal a clause option holds, or the value of the parameter it
    /// names, bound as a placeholder.
    fn opt_placeholder(
        &mut self,
        opts: &BTreeMap<String, Value>,
        key: &str,
        at: &str,
    ) -> R<Option<String>> {
        let Some(v) = opts.get(key) else {
            return Ok(None);
        };
        Ok(Some(match v {
            Value::String(s) => self.p(Param::from(s.as_str()), Type::Text),
            Value::Number(n) if n.is_i64() => {
                self.p(Param::Int(n.as_i64().unwrap_or(0)), Type::Int)
            }
            Value::Number(n) => self.p(Param::Double(n.as_f64().unwrap_or(0.0)), Type::Double),
            other => {
                let inner = crate::ast::clause_of(other).map_err(|m| err(at, m))?;
                if inner.op != "param" {
                    return Err(err(at, format!("option {key} is a literal or a param ref")));
                }
                let name = inner.ref_name().unwrap_or("");
                self.param_term(name, at)?.sql
            }
        }))
    }

    // ---------------------------------------------------------------- a set

    fn build_set(&mut self, name: &str) -> R<()> {
        let set = &self.ask.sets[name];
        let path = format!("sets.{name}");
        if set.grain == Grain::Group {
            return self.build_group(name);
        }
        if let Some(a) = &set.algebra {
            return self.build_algebra(name, a);
        }
        let base = self.base(set.grain, &path)?;
        let mut from = base.from.clone();
        let mut wheres: Vec<String> = base.standing.clone();
        let mut frame = Frame {
            has_subj: base.subj.is_some(),
            has_day: base.day.is_some(),
            has_prec: base.prec.is_some(),
            ..Frame::default()
        };
        let null = || "NULL".to_string();
        let mut projected: Vec<String> = vec![
            format!("{} AS k", base.key),
            format!("{} AS subj", base.subj.clone().unwrap_or_else(null)),
            format!("{} AS day", base.day.clone().unwrap_or_else(null)),
            format!("{} AS prec", base.prec.clone().unwrap_or_else(null)),
            format!(
                "{} AS session_k",
                base.session_k.clone().unwrap_or_else(null)
            ),
            format!("{} AS study_k", base.study_k.clone().unwrap_or_else(null)),
            format!("{} AS series_k", base.series_k.clone().unwrap_or_else(null)),
            format!("{} AS stack_k", base.stack_k.clone().unwrap_or_else(null)),
        ];
        let mut terms: Vec<(String, Term)> = Vec::new();
        // a source: its bindings and partners come along
        match &set.from {
            None => {}
            Some(Src::Set(s)) => {
                from.push_str(&format!(" JOIN {} x ON x.k = {}", cte_name(s), base.key));
                let src = self.frames.get(s).cloned().unwrap_or_default();
                let mut carried: BTreeSet<String> = BTreeSet::new();
                for (n, t) in &src.terms {
                    let col = &t.sql;
                    if col.starts_with("b_")
                        || col.starts_with("n_")
                        || col.starts_with("pick_")
                        || col.starts_with("o_")
                    {
                        for c in [Some(col.clone()), t.prec.clone(), t.ci.clone()]
                            .into_iter()
                            .flatten()
                        {
                            if carried.insert(c.clone()) {
                                projected.push(format!("x.{c} AS {c}"));
                            }
                        }
                        terms.push((n.clone(), t.clone()));
                    }
                }
                frame.bindings = src.bindings.clone();
                frame.of.clone_from(&src.of);
                frame.picked = src.picked;
            }
            Some(Src::Role(r)) => {
                if set.grain != Grain::Stack {
                    return Err(err(format!("{path}.from"), "a role is a stack set"));
                }
                let role = self.p(Param::from(r.as_str()), Type::Text);
                from.push_str(&format!(
                    " JOIN {} ps ON ps.stack_id = st.id JOIN {} p ON p.id = ps.pick_id AND p.role = {role} AND p.withdrawn_at IS NULL",
                    self.q("pick_stack"),
                    self.q("pick")
                ));
            }
            Some(Src::Handle { id, .. }) => {
                let hid: i64 = id.parse().map_err(|_| {
                    err(
                        format!("{path}.from"),
                        format!("handle {id} is not a number"),
                    )
                })?;
                let h = self.p(Param::Int(hid), Type::Int);
                from.push_str(&format!(
                    " JOIN {} hm ON hm.handle_id = {h} AND hm.key = {}",
                    self.q("handle_member"),
                    base.key
                ));
            }
            Some(Src::Values(v)) => {
                let decl = self.ask.values.get(v).ok_or_else(|| {
                    err(
                        format!("{path}.from"),
                        format!("values:{v} is not declared"),
                    )
                })?;
                let upload = self.p(Param::from(decl.upload.as_str()), Type::Text);
                from.push_str(&format!(
                    " JOIN {} vm ON vm.subject_id = su.id JOIN {} vs ON vs.id = vm.source_id AND vs.upload_id = {upload}",
                    self.q("values_member"),
                    self.q("values_source")
                ));
            }
            Some(Src::Selection { .. }) => {
                return Err(err(
                    format!("{path}.from"),
                    "a selection source lands with slice 7, the stored ask",
                ));
            }
        }
        // the ancestor
        if let Some(of) = &set.of {
            let ancestor = self
                .ask
                .sets
                .get(of)
                .ok_or_else(|| err(format!("{path}.of"), format!("no set {of}")))?;
            let on = match (ancestor.grain, set.grain) {
                (Grain::Cohort, _) => {
                    from.push_str(&format!(
                        " JOIN {} cm ON cm.subject_id = su.id AND cm.left_at IS NULL",
                        self.q("cohort_member")
                    ));
                    "a.k = cm.cohort_id".to_string()
                }
                (Grain::Subject, _) => "a.k = su.id".to_string(),
                (Grain::Session, Grain::Stack | Grain::Instance) => {
                    "a.k = scs.session_id".to_string()
                }
                (Grain::Stack, Grain::Instance) => "a.k = i.stack_id".to_string(),
                (g, h) => {
                    return Err(err(
                        format!("{path}.of"),
                        format!("{g} is not an ancestor of {h}"),
                    ));
                }
            };
            from.push_str(&format!(" JOIN {} a ON {on}", cte_name(of)));
            let anc = self.frames.get(of).cloned().unwrap_or_default();
            let mut carried: BTreeSet<String> = BTreeSet::new();
            for (n, t) in &anc.terms {
                if t.sql.starts_with("b_") || t.sql.starts_with("n_") {
                    let mine = format!("o_{}", t.sql);
                    if carried.insert(mine.clone()) {
                        projected.push(format!("a.{} AS {mine}", t.sql));
                    }
                    let prec = t.prec.as_ref().map(|p| {
                        let pc = format!("o_{p}");
                        if carried.insert(pc.clone()) {
                            projected.push(format!("a.{p} AS {pc}"));
                        }
                        pc
                    });
                    terms.push((
                        format!("{of}.{n}"),
                        Term {
                            sql: mine,
                            prec,
                            ci: None,
                            param: None,
                        },
                    ));
                }
            }
            if ancestor.grain == Grain::Cohort {
                projected.push("a.k AS cohort_k".into());
                terms.push(("cohort.id".into(), Term::plain("cohort_k".into())));
            }
            frame.of = Some(of.clone());
        }
        // the fields this set and its readers name, projected once
        let wanted = self.wanted_paths(name, set);
        for p in &wanted {
            if terms.iter().any(|(n, _)| n == p) {
                continue;
            }
            if let Some(t) = self.field_term(set, &frame, p) {
                let col = field_col(p);
                projected.push(format!("{} AS {col}", t.sql));
                let mut layered = Term::plain(col.clone());
                if let Some(pc) = &t.prec {
                    let pcol = format!("{col}__prec");
                    projected.push(format!("{pc} AS {pcol}"));
                    layered.prec = Some(pcol);
                }
                if let Some(ci) = &t.ci {
                    let ccol = format!("{col}__ci");
                    projected.push(format!("{ci} AS {ccol}"));
                    layered.ci = Some(ccol);
                }
                terms.push((p.clone(), layered));
            }
        }
        // the spine, by name
        if frame.has_day {
            terms.push((
                "day".into(),
                Term {
                    sql: "day".into(),
                    prec: frame.has_prec.then(|| "prec".to_string()),
                    ci: None,
                    param: None,
                },
            ));
        }
        terms.push((format!("{}.id", set.grain.name()), Term::plain("k".into())));
        if frame.has_subj && !terms.iter().any(|(n, _)| n == "subject.id") {
            terms.push(("subject.id".into(), Term::plain("subj".into())));
        }
        if set.grain == Grain::Cohort {
            terms.push(("cohort.id".into(), Term::plain("k".into())));
        }
        if frame.picked {
            for (n, c) in [
                ("pick.tied", "pick_tied"),
                ("pick.candidates", "pick_candidates"),
                ("pick.rank", "pick_rank"),
            ] {
                if !terms.iter().any(|(t, _)| t == n) {
                    terms.push((n.into(), Term::plain(c.into())));
                }
            }
        }
        let mut layer = format!("SELECT {} FROM {from}", projected.join(", "));
        if !wheres.is_empty() {
            layer.push_str(&format!(" WHERE {}", wheres.join(" AND ")));
        }
        wheres.clear();

        // near: the one row of a dated set of the same subject in a window
        for (i, n) in set.near.iter().enumerate() {
            layer = self.near_layer(
                name,
                set,
                n,
                &format!("{path}.near[{i}]"),
                layer,
                &mut terms,
            )?;
        }
        // attach: the one row of a descendant set picked per this grain
        for (i, a) in set.attach.iter().enumerate() {
            layer = self.attach_layer(set, a, &format!("{path}.attach[{i}]"), layer, &mut terms)?;
        }
        // has, before bind: a count becomes a binding, a bound count a predicate
        let mut has_preds: Vec<String> = Vec::new();
        for (i, h) in set.has.iter().enumerate() {
            let hp = format!("{path}.has[{i}]");
            let count =
                self.count_of(set, &h.set, h.window.as_ref(), h.on.as_deref(), &terms, &hp)?;
            if let Some(as_) = &h.as_ {
                let col = column_name(as_);
                layer = format!("SELECT q.*, {count} AS {col} FROM ({layer}) q");
                frame.bindings.push((as_.clone(), col.clone()));
                terms.push((as_.clone(), Term::plain(col)));
            }
            if let Some(min) = &h.min {
                let b = self.int_spec(min, &hp)?;
                has_preds.push(format!("{count} >= {b}"));
            }
            if let Some(max) = &h.max {
                let b = self.int_spec(max, &hp)?;
                has_preds.push(format!("{count} <= {b}"));
            }
        }
        if !has_preds.is_empty() {
            layer = format!(
                "SELECT q.* FROM ({layer}) q WHERE {}",
                has_preds.join(" AND ")
            );
        }
        // bind, one layer each so a later one reads an earlier one
        for (b, c) in &set.bind.0 {
            let bp = format!("{path}.bind.{b}");
            if c.op == "change" {
                layer = self.change_layer(set, b, c, &bp, layer, &mut terms, &mut frame)?;
                continue;
            }
            let expr = self.expr(c, &terms, "q", &bp)?;
            let col = column_name(b);
            layer = format!("SELECT q.*, {} AS {col} FROM ({layer}) q", expr.sql);
            frame.bindings.push((b.clone(), col.clone()));
            terms.push((
                b.clone(),
                Term {
                    sql: col,
                    prec: expr.prec,
                    ci: None,
                    param: None,
                },
            ));
        }
        // where
        if !set.where_.is_empty() {
            let mut preds = Vec::new();
            for (i, c) in set.where_.iter().enumerate() {
                preds.push(
                    self.expr(c, &terms, "q", &format!("{path}.where[{i}]"))?
                        .sql,
                );
            }
            layer = format!("SELECT q.* FROM ({layer}) q WHERE {}", preds.join(" AND "));
        }
        // pick
        if let Some(pk) = &set.pick {
            let pp = format!("{path}.pick");
            let partition = match pk.per {
                Grain::Subject => "subj",
                Grain::Session => "session_k",
                Grain::Stack => "stack_k",
                Grain::Cohort => "cohort_k",
                other => {
                    return Err(err(
                        pp.clone(),
                        format!("pick per {other} is not an ancestor"),
                    ));
                }
            };
            let mut projected_order = Vec::new();
            let mut order = Vec::new();
            for (i, o) in pk.by.iter().enumerate() {
                let e = self.expr(&o.0, &terms, "q", &format!("{pp}.by[{i}]"))?;
                projected_order.push(format!("{} AS pk_{i}", e.sql));
                order.push(self.sql.order_term(&format!("q.pk_{i}"), o.1));
            }
            layer = format!(
                "SELECT q.*, {} FROM ({layer}) q",
                projected_order.join(", ")
            );
            let by = order.join(", ");
            let n = match &pk.n {
                Some(spec) => self.int_spec(spec, &pp)?,
                None => "1".into(),
            };
            layer = format!(
                "SELECT q.*, ROW_NUMBER() OVER (PARTITION BY q.{partition} ORDER BY {by}, q.k) AS pick_rn, \
                 RANK() OVER (PARTITION BY q.{partition} ORDER BY {by}) AS pick_rank, \
                 COUNT(*) OVER (PARTITION BY q.{partition}) AS pick_candidates FROM ({layer}) q"
            );
            layer = format!(
                "SELECT q.*, CASE WHEN SUM(CASE WHEN q.pick_rank = 1 THEN 1 ELSE 0 END) OVER (PARTITION BY q.{partition}) > 1 \
                 THEN 1 ELSE 0 END AS pick_tied FROM ({layer}) q"
            );
            layer = format!("SELECT q.* FROM ({layer}) q WHERE q.pick_rn <= {n}");
            frame.picked = true;
            for (n, c) in [
                ("pick.tied", "pick_tied"),
                ("pick.candidates", "pick_candidates"),
                ("pick.rank", "pick_rank"),
            ] {
                if !terms.iter().any(|(t, _)| t == n) {
                    terms.push((n.into(), Term::plain(c.into())));
                }
            }
        }
        frame.terms = terms;
        self.frames.insert(name.to_string(), frame);
        self.push_cte(name, &layer);
        Ok(())
    }

    /// A partner's columns, carried into this set under `n_<as>__`.
    fn carry_partner(
        &self,
        as_: &str,
        partner: &Frame,
        alias: &str,
        projected: &mut Vec<String>,
        terms: &mut Vec<(String, Term)>,
    ) {
        let mut carried: BTreeSet<String> = BTreeSet::new();
        let prefix = format!("n_{}__", as_.replace('.', "__"));
        for (n, t) in &partner.terms {
            let mut carry = |c: &str| -> String {
                let mine = format!("{prefix}{c}");
                if carried.insert(mine.clone()) {
                    projected.push(format!("{alias}.{c} AS {mine}"));
                }
                mine
            };
            let sql = carry(&t.sql);
            let prec = t.prec.as_deref().map(&mut carry);
            let ci = t.ci.as_deref().map(&mut carry);
            terms.push((
                format!("{as_}.{n}"),
                Term {
                    sql,
                    prec,
                    ci,
                    param: None,
                },
            ));
        }
        for (n, c) in [("date", "day"), ("precision", "prec")] {
            let mine = format!("{prefix}{c}");
            if carried.insert(mine.clone()) {
                projected.push(format!("{alias}.{c} AS {mine}"));
            }
            let prec = (n == "date" && partner.has_prec).then(|| format!("{prefix}prec"));
            terms.push((
                format!("{as_}.{n}"),
                Term {
                    sql: mine,
                    prec,
                    ci: None,
                    param: None,
                },
            ));
        }
        let key = format!("{prefix}k");
        if carried.insert(key.clone()) {
            projected.push(format!("{alias}.k AS {key}"));
        }
    }

    /// One `near`: three layers, the join with the window and the ranks,
    /// the tie from the ranks, the cut.
    #[allow(clippy::too_many_arguments)]
    fn near_layer(
        &mut self,
        set_name: &str,
        set: &Set,
        n: &crate::ast::Near,
        at: &str,
        layer: String,
        terms: &mut Vec<(String, Term)>,
    ) -> R<String> {
        let _ = set_name;
        let partner = self.frames.get(&n.set).cloned().unwrap_or_default();
        if !partner.has_day || !set.grain.dated() {
            return Err(err(at, "near stays between two dated sets"));
        }
        let (lo, hi) = window_days(&n.window, at)?;
        let anchor = match &n.on {
            Some(on) => {
                let t = terms
                    .iter()
                    .find(|(x, _)| x == on)
                    .map(|(_, t)| t.clone())
                    .ok_or_else(|| err(at, format!("{on} is not bound")))?;
                format!("q.{}", t.sql)
            }
            None => "q.day".to_string(),
        };
        let p_first = "p.day".to_string();
        let p_last = if partner.has_prec {
            self.sql.interval_end("p.day", "p.prec")
        } else {
            "p.day".to_string()
        };
        let edge = |b: &Builder, bound: i64| -> String {
            if b.sql.is_pg() {
                format!("({anchor} + ({bound}))")
            } else {
                format!("date({anchor}, '{bound:+} days')")
            }
        };
        let mut on = "p.subj = q.subj".to_string();
        if let Some(lo) = lo {
            let e = edge(self, lo);
            if n.strict {
                on.push_str(&format!(" AND {p_first} >= {e}"));
            } else {
                on.push_str(&format!(" AND {p_last} >= {e}"));
            }
        }
        if let Some(hi) = hi {
            let e = edge(self, hi);
            if n.strict {
                on.push_str(&format!(" AND {p_last} <= {e}"));
            } else {
                on.push_str(&format!(" AND {p_first} <= {e}"));
            }
        }
        let prefix = format!("n_{}__", n.as_.replace('.', "__"));
        // the signed distance to the nearest edge of the partner's interval
        let offset = format!(
            "CASE WHEN {anchor} < {p_first} THEN {} WHEN {anchor} > {p_last} THEN {} ELSE 0 END",
            self.sql.days_between(&p_first, &anchor),
            self.sql.days_between(&p_last, &anchor)
        );
        let tie = match n.tie {
            Some(crate::ast::Tie::Later) => "DESC",
            _ => "ASC",
        };
        let rank = match n.policy {
            crate::ast::Policy::Nearest => format!("ABS({offset}) ASC, p.day {tie}"),
            crate::ast::Policy::First | crate::ast::Policy::Any => "p.day ASC".to_string(),
            crate::ast::Policy::Last => "p.day DESC".to_string(),
            crate::ast::Policy::Best => {
                // the order reads this set's terms as q and the partner's as p
                let mut both: Vec<(String, Term)> = terms
                    .iter()
                    .map(|(x, t)| {
                        (
                            x.clone(),
                            Term {
                                sql: format!("q.{}", t.sql),
                                prec: t.prec.as_ref().map(|p| format!("q.{p}")),
                                ci: None,
                                param: None,
                            },
                        )
                    })
                    .collect();
                for (x, t) in &partner.terms {
                    both.push((
                        format!("{}.{x}", n.as_),
                        Term {
                            sql: format!("p.{}", t.sql),
                            prec: t.prec.as_ref().map(|p| format!("p.{p}")),
                            ci: None,
                            param: None,
                        },
                    ));
                }
                both.push((format!("{}.date", n.as_), Term::plain(p_first.clone())));
                both.push((
                    format!("{}.offset_days", n.as_),
                    Term::plain(offset.clone()),
                ));
                let mut parts = Vec::new();
                for (i, o) in n.order.iter().enumerate() {
                    let e = self.expr(&o.0, &both, "", &format!("{at}.order[{i}]"))?;
                    parts.push(self.sql.order_term(&e.sql, o.1));
                }
                parts.join(", ")
            }
        };
        let mut projected: Vec<String> = vec!["q.*".into()];
        self.carry_partner(&n.as_, &partner, "p", &mut projected, terms);
        projected.push(format!("{offset} AS {prefix}offset"));
        terms.push((
            format!("{}.offset_days", n.as_),
            Term::plain(format!("{prefix}offset")),
        ));
        projected.push(format!(
            "ROW_NUMBER() OVER (PARTITION BY q.k ORDER BY {rank}, p.k) AS {prefix}rn"
        ));
        projected.push(format!(
            "RANK() OVER (PARTITION BY q.k ORDER BY {rank}) AS {prefix}rank"
        ));
        projected.push(format!(
            "COUNT(p.k) OVER (PARTITION BY q.k) AS {prefix}cand"
        ));
        terms.push((
            format!("{}.candidates", n.as_),
            Term::plain(format!("{prefix}cand")),
        ));
        let mut layer = format!(
            "SELECT {} FROM ({layer}) q LEFT JOIN {} p ON {on}",
            projected.join(", "),
            cte_name(&n.set)
        );
        layer = format!(
            "SELECT q.*, CASE WHEN SUM(CASE WHEN q.{prefix}rank = 1 AND q.{prefix}k IS NOT NULL THEN 1 ELSE 0 END) OVER (PARTITION BY q.k) > 1 THEN 1 ELSE 0 END AS {prefix}tied FROM ({layer}) q"
        );
        terms.push((
            format!("{}.tied", n.as_),
            Term::plain(format!("{prefix}tied")),
        ));
        let mut cut = format!("q.{prefix}rn = 1");
        if !n.optional {
            cut.push_str(&format!(" AND q.{prefix}k IS NOT NULL"));
        }
        Ok(format!("SELECT q.* FROM ({layer}) q WHERE {cut}"))
    }

    /// One `attach`: the partner joined on this grain's key in it.
    fn attach_layer(
        &mut self,
        set: &Set,
        a: &crate::ast::Attach,
        at: &str,
        layer: String,
        terms: &mut Vec<(String, Term)>,
    ) -> R<String> {
        let partner = self.frames.get(&a.set).cloned().unwrap_or_default();
        let link = Self::link(&partner, set.grain, "p", at)?;
        let mut projected: Vec<String> = vec!["q.*".into()];
        self.carry_partner(&a.as_, &partner, "p", &mut projected, terms);
        let prefix = format!("n_{}__", a.as_.replace('.', "__"));
        let mut out = format!(
            "SELECT {} FROM ({layer}) q LEFT JOIN {} p ON {link} = q.k",
            projected.join(", "),
            cte_name(&a.set)
        );
        if !a.optional {
            out = format!("SELECT q.* FROM ({out}) q WHERE q.{prefix}k IS NOT NULL");
        }
        Ok(out)
    }

    /// The count of a child set's rows per row of this set, windowed when
    /// asked, as a correlated subquery.
    fn count_of(
        &mut self,
        set: &Set,
        child_name: &str,
        window: Option<&crate::ast::WindowSpec>,
        on: Option<&str>,
        terms: &[(String, Term)],
        at: &str,
    ) -> R<String> {
        let child = self.frames.get(child_name).cloned().unwrap_or_default();
        let child_set = &self.ask.sets[child_name];
        if child_set.grain == Grain::Group {
            let key_path = format!("{}.id", set.grain.name());
            let (_, gcol) = child
                .group_by
                .iter()
                .find(|(p, _)| p == &key_path)
                .ok_or_else(|| err(at, "the group is not keyed by this grain"))?;
            return Ok(format!(
                "(SELECT COUNT(*) FROM {} ch WHERE ch.{gcol} = q.k)",
                cte_name(child_name)
            ));
        }
        if child_set.grain == set.grain {
            return Ok(format!(
                "(SELECT COUNT(*) FROM {} ch WHERE ch.k = q.k)",
                cte_name(child_name)
            ));
        }
        let link = Self::link(&child, set.grain, "ch", at)?;
        let mut inner = format!(
            "SELECT COUNT(*) FROM {} ch WHERE {link} = q.k",
            cte_name(child_name)
        );
        if let Some(w) = window {
            let (lo, hi) = window_days(w, at)?;
            let anchor = match on {
                Some(on) => terms
                    .iter()
                    .find(|(n, _)| n == on)
                    .map(|(_, t)| format!("q.{}", t.sql))
                    .ok_or_else(|| err(at, format!("{on} is not bound")))?,
                None => "q.day".to_string(),
            };
            let delta = self.sql.days_between("ch.day", &anchor);
            if let Some(lo) = lo {
                inner.push_str(&format!(" AND {delta} >= {lo}"));
            }
            if let Some(hi) = hi {
                inner.push_str(&format!(" AND {delta} <= {hi}"));
            }
        }
        Ok(format!("({inner})"))
    }

    /// `change` (§4.3, §5.4): the first row of the `to` value that follows
    /// a row of the `from` value, adjacent by default, over a subject's
    /// course rows or the rows of an event kind, joined per subject.
    #[allow(clippy::too_many_arguments)]
    fn change_layer(
        &mut self,
        set: &Set,
        binding: &str,
        c: &Clause,
        at: &str,
        layer: String,
        terms: &mut Vec<(String, Term)>,
        frame: &mut Frame,
    ) -> R<String> {
        if set.grain != Grain::Subject {
            return Err(err(at, "change is a subject's history"));
        }
        let of = c
            .opts
            .get("of")
            .and_then(Value::as_str)
            .unwrap_or("course")
            .to_string();
        let adjacent = c
            .opts
            .get("adjacent")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        // the rows, in text order of their placeholders
        let rows = if of == "course" {
            let mut sql = format!(
                "SELECT sdt.id AS id, sd.subject_id AS subject_id, sdt.assigned_on AS date, \
                 COALESCE(sdt.assigned_on_precision, 'day') AS prec, dt.name AS value \
                 FROM {} sdt JOIN {} sd ON sd.id = sdt.subject_disease_id \
                 JOIN {} dt ON dt.id = sdt.disease_type_id JOIN {} d ON d.id = sd.disease_id \
                 WHERE sdt.superseded_by IS NULL AND sd.superseded_by IS NULL AND sdt.assigned_on IS NOT NULL",
                self.q("subject_disease_type"),
                self.q("subject_disease"),
                self.q("disease_type"),
                self.q("disease")
            );
            if let Some(d) = self.opt_placeholder(&c.opts, "disease", at)? {
                sql.push_str(&format!(" AND d.name = {d}"));
            }
            sql
        } else {
            let kind = self.p(Param::from(of.as_str()), Type::Text);
            format!(
                "SELECT e.id AS id, e.subject_id AS subject_id, e.event_date AS date, \
                 COALESCE(e.event_date_precision, 'day') AS prec, e.value AS value \
                 FROM {} e JOIN {} ot ON ot.id = e.observation_type_id \
                 WHERE ot.name = {kind} AND e.superseded_by IS NULL",
                self.q("event"),
                self.q("observation_type")
            )
        };
        let from_v = self
            .opt_placeholder(&c.opts, "from", at)?
            .ok_or_else(|| err(at, "change needs from"))?;
        let seq = "PARTITION BY r.subject_id ORDER BY r.date, r.id";
        let staged = format!(
            "SELECT r.subject_id, r.date AS to_date, r.prec AS to_prec, r.value, r.id, \
             LAG(r.date) OVER ({seq}) AS from_date, LAG(r.value) OVER ({seq}) AS prev_value, \
             MAX(CASE WHEN r.value = {from_v} THEN 1 ELSE 0 END) OVER ({seq} ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING) AS seen_from \
             FROM ({rows}) r"
        );
        let to_v = self
            .opt_placeholder(&c.opts, "to", at)?
            .ok_or_else(|| err(at, "change needs to"))?;
        let mut hit = format!("SELECT t.* FROM ({staged}) t WHERE t.value = {to_v}");
        if adjacent {
            let from_again = self
                .opt_placeholder(&c.opts, "from", at)?
                .unwrap_or_default();
            hit.push_str(&format!(" AND t.prev_value = {from_again}"));
        } else {
            hit.push_str(" AND t.seen_from = 1");
        }
        let first = format!(
            "SELECT h.* FROM (SELECT t.*, ROW_NUMBER() OVER (PARTITION BY t.subject_id ORDER BY t.to_date, t.id) AS rn FROM ({hit}) t) h WHERE h.rn = 1"
        );
        let col = column_name(binding);
        let gap = self.sql.days_between("ch.to_date", "ch.from_date");
        let out = format!(
            "SELECT q.*, ch.to_date AS {col}, ch.to_date AS {col}__to_date, ch.from_date AS {col}__from_date, \
             ch.to_prec AS {col}__precision, {gap} AS {col}__gap_days \
             FROM ({layer}) q LEFT JOIN ({first}) ch ON ch.subject_id = q.subj"
        );
        frame.bindings.push((binding.to_string(), col.clone()));
        let dated = |c: String, prec: Option<String>| Term {
            sql: c,
            prec,
            ci: None,
            param: None,
        };
        terms.push((
            binding.to_string(),
            dated(col.clone(), Some(format!("{col}__precision"))),
        ));
        terms.push((
            format!("{binding}.to_date"),
            dated(format!("{col}__to_date"), Some(format!("{col}__precision"))),
        ));
        terms.push((
            format!("{binding}.from_date"),
            Term::plain(format!("{col}__from_date")),
        ));
        terms.push((
            format!("{binding}.precision"),
            Term::plain(format!("{col}__precision")),
        ));
        terms.push((
            format!("{binding}.gap_days"),
            Term::plain(format!("{col}__gap_days")),
        ));
        Ok(out)
    }

    fn build_group(&mut self, name: &str) -> R<()> {
        let set = &self.ask.sets[name];
        let path = format!("sets.{name}");
        let g = set
            .group
            .as_ref()
            .ok_or_else(|| err(&path, "a group set needs group"))?;
        let child = self.frames.get(&g.of).cloned().unwrap_or_default();
        let child_terms = child.terms.clone();
        let mut cols: Vec<String> = Vec::new();
        let mut group_cols: Vec<String> = Vec::new();
        let mut frame = Frame::default();
        for (i, c) in g.by.iter().enumerate() {
            let e = self.expr(c, &child_terms, "ch", &format!("{path}.group.by[{i}]"))?;
            let col = format!("g{i}");
            cols.push(format!("{} AS {col}", e.sql));
            group_cols.push(e.sql.clone());
            let label = c
                .ref_name()
                .map(str::to_string)
                .unwrap_or_else(|| format!("by{i}"));
            frame.group_by.push((label, col));
        }
        cols.push("COUNT(*) AS _rows".into());
        cols.push(if child.has_subj {
            "COUNT(DISTINCT ch.subj) AS _subjects".into()
        } else {
            "COUNT(DISTINCT ch.k) AS _subjects".into()
        });
        frame.bindings.push(("_rows".into(), "_rows".into()));
        frame
            .bindings
            .push(("_subjects".into(), "_subjects".into()));
        let mut terms: Vec<(String, Term)> = vec![
            ("_rows".into(), Term::plain("_rows".into())),
            ("_subjects".into(), Term::plain("_subjects".into())),
        ];
        for (label, col) in &frame.group_by {
            terms.push((label.clone(), Term::plain(col.clone())));
        }
        let mut post: Vec<(String, String)> = Vec::new();
        for (b, c) in &set.bind.0 {
            let bp = format!("{path}.bind.{b}");
            let col = column_name(b);
            if AGGREGATES.contains(&c.op.as_str()) {
                let target = c.opts.get("set").and_then(Value::as_str).unwrap_or("");
                if target != g.of {
                    return Err(err(
                        bp,
                        format!("a group aggregates its own child {}, not {target}", g.of),
                    ));
                }
                let inner = match c.args.first() {
                    Some(Arg::Clause(inner)) => {
                        Some(self.expr(inner, &child_terms, "ch", &bp)?.sql)
                    }
                    Some(_) => return Err(err(bp, "an aggregate's argument is a clause")),
                    None => None,
                };
                cols.push(format!(
                    "{} AS {col}",
                    self.aggregate(&c.op, inner.as_deref(), &bp)?
                ));
            } else {
                post.push((b.clone(), col.clone()));
                continue;
            }
            frame.bindings.push((b.clone(), col.clone()));
            terms.push((b.clone(), Term::plain(col)));
        }
        let mut layer = format!(
            "SELECT {} FROM {} ch GROUP BY {}",
            cols.join(", "),
            cte_name(&g.of),
            group_cols.join(", ")
        );
        for (b, col) in post {
            let c = set.bind.get(&b).expect("a binding");
            let e = self.expr(c, &terms, "q", &format!("{path}.bind.{b}"))?;
            layer = format!("SELECT q.*, {} AS {col} FROM ({layer}) q", e.sql);
            frame.bindings.push((b.clone(), col.clone()));
            terms.push((b, Term::plain(col)));
        }
        let order: Vec<String> = frame
            .group_by
            .iter()
            .map(|(_, c)| format!("q.{c}"))
            .collect();
        layer = format!(
            "SELECT q.*, ROW_NUMBER() OVER (ORDER BY {}) AS k, NULL AS subj FROM ({layer}) q",
            order.join(", ")
        );
        if !set.where_.is_empty() {
            let mut preds = Vec::new();
            for (i, c) in set.where_.iter().enumerate() {
                preds.push(
                    self.expr(c, &terms, "q", &format!("{path}.where[{i}]"))?
                        .sql,
                );
            }
            layer = format!("SELECT q.* FROM ({layer}) q WHERE {}", preds.join(" AND "));
        }
        terms.push(("group.id".into(), Term::plain("k".into())));
        frame.terms = terms;
        self.frames.insert(name.to_string(), frame);
        self.push_cte(name, &layer);
        Ok(())
    }

    fn aggregate(&self, op: &str, inner: Option<&str>, path: &str) -> R<String> {
        let x = inner.unwrap_or("ch.k");
        Ok(match op {
            "count" => match inner {
                None => "COUNT(*)".into(),
                Some(i) => format!("COUNT({i})"),
            },
            "distinct" => format!("COUNT(DISTINCT {x})"),
            "min" => format!("MIN({x})"),
            "max" => format!("MAX({x})"),
            "sum" => self.sql.as_bigint(&format!("SUM({x})")),
            "avg" => self.sql.as_double(&format!("AVG({x})")),
            "list" => self.sql.sorted_list(x),
            other => return Err(err(path, format!("{other} is not an aggregate"))),
        })
    }

    fn build_algebra(&mut self, name: &str, a: &crate::ast::Algebra) -> R<()> {
        let path = format!("sets.{name}.algebra");
        let left = a.sets.first().ok_or_else(|| err(&path, "no operand"))?;
        let lf = self.frames.get(left).cloned().unwrap_or_default();
        let mut frame = lf.clone();
        let layer = match a.op {
            AlgOp::Union => {
                let mut common: Vec<(String, String)> = lf.bindings.clone();
                for other in a.sets.iter().skip(1) {
                    let of = self.frames.get(other).cloned().unwrap_or_default();
                    common.retain(|(b, _)| of.bindings.iter().any(|(ob, _)| ob == b));
                }
                frame.bindings = common.clone();
                frame.picked = false;
                frame.terms.retain(|(n, t)| {
                    !t.sql.starts_with("b_")
                        && !t.sql.starts_with("n_")
                        && !t.sql.starts_with("pick_")
                        || common.iter().any(|(b, _)| b == n)
                });
                let mut parts = Vec::new();
                for s in &a.sets {
                    let mut cols: Vec<String> = SPINE.iter().map(|c| format!("l.{c}")).collect();
                    let of = self.frames.get(s).cloned().unwrap_or_default();
                    // the fields the left exposes, by column; a right operand
                    // that lacks one yields NULL
                    for (n, t) in &lf.terms {
                        if t.sql.starts_with("f_") {
                            if of.terms.iter().any(|(m, _)| m == n) {
                                cols.push(format!("l.{} AS {}", t.sql, t.sql));
                            } else {
                                cols.push(format!("NULL AS {}", t.sql));
                            }
                        }
                    }
                    for (_, col) in &common {
                        cols.push(format!("l.{col}"));
                    }
                    if a.tag.is_some() {
                        cols.push(format!("'{s}' AS tag"));
                    }
                    parts.push(format!("SELECT {} FROM {} l", cols.join(", "), cte_name(s)));
                }
                if let Some(t) = &a.tag {
                    frame.bindings.push((t.clone(), "tag".into()));
                    frame.terms.push((t.clone(), Term::plain("tag".into())));
                    parts.join(" UNION ALL ")
                } else {
                    parts.join(" UNION ")
                }
            }
            AlgOp::Intersect | AlgOp::Except => {
                let keyword = if a.op == AlgOp::Intersect {
                    "EXISTS"
                } else {
                    "NOT EXISTS"
                };
                let mut preds = Vec::new();
                for s in a.sets.iter().skip(1) {
                    preds.push(format!(
                        "{keyword} (SELECT 1 FROM {} r WHERE r.k = l.k)",
                        cte_name(s)
                    ));
                }
                format!(
                    "SELECT l.* FROM {} l WHERE {}",
                    cte_name(left),
                    preds.join(" AND ")
                )
            }
        };
        self.frames.insert(name.to_string(), frame);
        self.push_cte(name, &layer);
        Ok(())
    }

    // ------------------------------------------------------------ expressions

    fn term(&self, terms: &[(String, Term)], alias: &str, path: &str, at: &str) -> R<Term> {
        let t = terms
            .iter()
            .find(|(n, _)| n == path)
            .map(|(_, t)| t.clone())
            .ok_or_else(|| err(at, format!("{path} is not projected in this layer")))?;
        let dot = |s: &str| {
            if alias.is_empty() {
                s.to_string()
            } else {
                format!("{alias}.{s}")
            }
        };
        Ok(Term {
            sql: dot(&t.sql),
            prec: t.prec.as_deref().map(dot),
            ci: t.ci.as_deref().map(dot),
            param: None,
        })
    }

    fn literal(&mut self, a: &Arg, at: &str) -> R<Term> {
        Ok(Term::plain(match a {
            Arg::Text(t) => self.p(Param::from(t.as_str()), Type::Text),
            Arg::Int(i) => self.p(Param::Int(*i), Type::Int),
            Arg::Number(n) => self.p(Param::Double(*n), Type::Double),
            Arg::Bool(b) => {
                if *b {
                    "1".into()
                } else {
                    "0".into()
                }
            }
            Arg::Null => "NULL".into(),
            Arg::List(_) => return Err(err(at, "a list is an argument of in, not a value")),
            Arg::Clause(_) => unreachable!(),
        }))
    }

    fn param_term(&mut self, name: &str, at: &str) -> R<Term> {
        let decl = self
            .ask
            .params
            .get(name)
            .ok_or_else(|| err(at, format!("no parameter {name}")))?;
        let v = decl
            .value
            .clone()
            .ok_or_else(|| err(at, format!("parameter {name} has no value")))?;
        let (param, ty) = match (decl.type_, &v) {
            (ParamType::Text | ParamType::Cohort, Value::String(s)) => {
                (Param::from(s.as_str()), Type::Text)
            }
            (ParamType::Integer, Value::Number(n)) => (
                Param::Int(
                    n.as_i64()
                        .ok_or_else(|| err(at, format!("parameter {name} is not an integer")))?,
                ),
                Type::Int,
            ),
            (ParamType::Number, Value::Number(n)) => {
                (Param::Double(n.as_f64().unwrap_or(0.0)), Type::Double)
            }
            (ParamType::Date, Value::String(s)) => (Param::from(s.as_str()), Type::Date),
            (ParamType::List, _) => {
                return Err(err(
                    at,
                    format!("parameter {name} is a list; it goes with in"),
                ));
            }
            _ => {
                return Err(err(
                    at,
                    format!("parameter {name} does not hold a value of its type"),
                ));
            }
        };
        let ph = self.p(param.clone(), ty);
        Ok(Term {
            sql: ph,
            prec: None,
            ci: None,
            param: Some((param, ty)),
        })
    }

    fn arg(&mut self, a: &Arg, terms: &[(String, Term)], alias: &str, at: &str) -> R<Term> {
        match a {
            Arg::Clause(c) => self.expr(c, terms, alias, at),
            other => self.literal(other, at),
        }
    }

    fn list_placeholders(&mut self, a: &Arg, at: &str) -> R<String> {
        let items: Vec<Param> = match a {
            Arg::List(items) => items
                .iter()
                .map(|i| match i {
                    Arg::Text(t) => Ok(Param::from(t.as_str())),
                    Arg::Int(n) => Ok(Param::Int(*n)),
                    Arg::Number(n) => Ok(Param::Double(*n)),
                    _ => Err(err(at, "a list holds literals")),
                })
                .collect::<R<_>>()?,
            Arg::Clause(c) if c.op == "param" => {
                let name = c.ref_name().unwrap_or("");
                let decl = self
                    .ask
                    .params
                    .get(name)
                    .ok_or_else(|| err(at, format!("no parameter {name}")))?;
                match &decl.value {
                    Some(Value::Array(items)) => items
                        .iter()
                        .map(|v| match v {
                            Value::String(s) => Ok(Param::from(s.as_str())),
                            Value::Number(n) if n.is_i64() => {
                                Ok(Param::Int(n.as_i64().unwrap_or(0)))
                            }
                            Value::Number(n) => Ok(Param::Double(n.as_f64().unwrap_or(0.0))),
                            _ => Err(err(
                                at,
                                format!("parameter {name} holds a value that is not a literal"),
                            )),
                        })
                        .collect::<R<_>>()?,
                    _ => return Err(err(at, format!("parameter {name} is not a list"))),
                }
            }
            _ => return Err(err(at, "in takes a list or a list parameter")),
        };
        if items.is_empty() {
            return Ok("(NULL)".into());
        }
        let mut ph = Vec::with_capacity(items.len());
        for i in items {
            let ty = match &i {
                Param::Int(_) => Type::Int,
                Param::Double(_) => Type::Double,
                _ => Type::Text,
            };
            ph.push(self.p(i, ty));
        }
        Ok(format!("({})", ph.join(", ")))
    }

    /// One side of a comparison, used once more: the first use is the
    /// term's own text, a later use binds a parameter again; `end` asks
    /// for the last day of a coarse date's interval.
    fn side(&mut self, t: &Term, uses: &mut u32, end: bool) -> String {
        *uses += 1;
        let d = if *uses == 1 {
            t.sql.clone()
        } else {
            self.again(t)
        };
        match (&t.prec, end) {
            (Some(p), true) => self.sql.interval_end(&d, p),
            _ => d,
        }
    }

    /// A comparison, coarse dates included (§5.3): forgiving by default,
    /// the certain reading under `strict: true`.
    fn compare(&mut self, op: &str, a: Term, b: Term, strict: bool) -> String {
        if a.prec.is_none() && b.prec.is_none() {
            return format!("({} {op} {})", a.sql, b.sql);
        }
        let (mut ua, mut ub) = (0u32, 0u32);
        match (op, strict) {
            (">", false) | (">=", false) => {
                let l = self.side(&a, &mut ua, true);
                let r = self.side(&b, &mut ub, false);
                format!("({l} {op} {r})")
            }
            (">", true) | (">=", true) => {
                let l = self.side(&a, &mut ua, false);
                let r = self.side(&b, &mut ub, true);
                format!("({l} {op} {r})")
            }
            ("<", false) | ("<=", false) => {
                let l = self.side(&a, &mut ua, false);
                let r = self.side(&b, &mut ub, true);
                format!("({l} {op} {r})")
            }
            ("<", true) | ("<=", true) => {
                let l = self.side(&a, &mut ua, true);
                let r = self.side(&b, &mut ub, false);
                format!("({l} {op} {r})")
            }
            ("=", false) | ("<>", false) => {
                let a1 = self.side(&a, &mut ua, false);
                let b2 = self.side(&b, &mut ub, true);
                let a2 = self.side(&a, &mut ua, true);
                let b1 = self.side(&b, &mut ub, false);
                let overlap = format!("({a1} <= {b2} AND {a2} >= {b1})");
                if op == "=" {
                    overlap
                } else {
                    format!("NOT {overlap}")
                }
            }
            ("=", true) => {
                let mut day = Vec::new();
                if let Some(p) = &a.prec {
                    day.push(format!("{p} = 'day'"));
                }
                if let Some(p) = &b.prec {
                    day.push(format!("{p} = 'day'"));
                }
                let l = self.side(&a, &mut ua, false);
                let r = self.side(&b, &mut ub, false);
                format!("({} AND {l} = {r})", day.join(" AND "))
            }
            _ => {
                let l = self.side(&a, &mut ua, false);
                let r = self.side(&b, &mut ub, false);
                format!("({l} {op} {r})")
            }
        }
    }

    /// A derived field (§4.3), from the fields its inputs project.
    fn derived(&mut self, c: &Clause, terms: &[(String, Term)], alias: &str, at: &str) -> R<Term> {
        let name = c
            .ref_name()
            .ok_or_else(|| err(at, "a derived ref names a field"))?;
        let col =
            |b: &Builder, path: &str| -> R<String> { Ok(b.term(terms, alias, path, at)?.sql) };
        let plain = |s: String| Ok(Term::plain(s));
        match name {
            "acquisition_type" => {
                let filled = col(self, "acquisition_type_filled")?;
                let read = col(self, "mr_acquisition_type")?;
                plain(format!("COALESCE({filled}, {read})"))
            }
            "field_strength" => plain(col(self, "field_strength_tesla")?),
            "study_day" => {
                let k = if alias.is_empty() {
                    "day".to_string()
                } else {
                    format!("{alias}.day")
                };
                Ok(Term {
                    sql: k,
                    prec: None,
                    ci: None,
                    param: None,
                })
            }
            "voxel" | "voxel_max" => {
                let third = c
                    .opts
                    .get("third")
                    .and_then(Value::as_str)
                    .unwrap_or("slice_thickness");
                let (r, cc, t) = (
                    col(self, "pixel_spacing_row")?,
                    col(self, "pixel_spacing_col")?,
                    col(self, third)?,
                );
                let inner = self.sql.greatest(&r, &cc);
                plain(self.sql.greatest(&inner, &t))
            }
            "voxel_min" => {
                let (r, cc, t) = (
                    col(self, "pixel_spacing_row")?,
                    col(self, "pixel_spacing_col")?,
                    col(self, "slice_thickness")?,
                );
                let inner = self.sql.least(&r, &cc);
                plain(self.sql.least(&inner, &t))
            }
            "resolution" => {
                let (r, cc, t) = (
                    col(self, "pixel_spacing_row")?,
                    col(self, "pixel_spacing_col")?,
                    col(self, "slice_thickness")?,
                );
                let s = self.sql;
                let part = |x: &str| format!("CAST({} AS TEXT)", s.rounded_key(x, 2));
                plain(format!(
                    "({} || 'x' || {} || 'x' || {})",
                    part(&r),
                    part(&cc),
                    part(&t)
                ))
            }
            "signature" => {
                let level = c
                    .opts
                    .get("level")
                    .and_then(Value::as_str)
                    .unwrap_or("loose")
                    .to_string();
                let spec = self
                    .ctx
                    .names
                    .level_spec(&level)
                    .ok_or_else(|| err(at, format!("{level} is not a comparability level")))?;
                let k = if alias.is_empty() {
                    "k".to_string()
                } else {
                    format!("{alias}.k")
                };
                let mut parts: Vec<String> = Vec::new();
                for m in &spec.exact {
                    parts.push(self.signature_member(m, None, &k, terms, alias, at)?);
                }
                for (m, step) in &spec.rounded {
                    parts.push(self.signature_member(m, Some(*step), &k, terms, alias, at)?);
                }
                plain(format!("({})", parts.join(" || '|' || ")))
            }
            "course" => {
                let subj = if alias.is_empty() {
                    "subj".to_string()
                } else {
                    format!("{alias}.subj")
                };
                let mut sql = format!(
                    "(SELECT dt.name FROM {} sdt JOIN {} sd ON sd.id = sdt.subject_disease_id \
                     JOIN {} dt ON dt.id = sdt.disease_type_id JOIN {} d ON d.id = sd.disease_id \
                     WHERE sd.subject_id = {subj} AND sdt.superseded_by IS NULL AND sd.superseded_by IS NULL",
                    self.q("subject_disease_type"),
                    self.q("subject_disease"),
                    self.q("disease_type"),
                    self.q("disease")
                );
                if let Some(d) = self.opt_placeholder(&c.opts, "disease", at)? {
                    sql.push_str(&format!(" AND d.name = {d}"));
                }
                sql.push_str(" ORDER BY sdt.assigned_on DESC, sdt.id DESC LIMIT 1)");
                plain(sql)
            }
            other => Err(err(at, format!("{other} is not a derived field"))),
        }
    }

    /// One member of a level's signature, as text that is the same on both
    /// backends: an axis as its sorted values, the acquisition type and the
    /// orientation as read, a physics number as an integer at a step.
    fn signature_member(
        &mut self,
        member: &str,
        step: Option<f64>,
        key: &str,
        terms: &[(String, Term)],
        alias: &str,
        at: &str,
    ) -> R<String> {
        if self.ctx.names.axis_values(member).is_some() {
            let ax = self.p(Param::from(member), Type::Text);
            let list = self.sql.sorted_list("ax.value");
            return Ok(format!(
                "COALESCE((SELECT {list} FROM {} ax WHERE ax.stack_id = {key} AND ax.axis = {ax}), '')",
                self.q("classification_axis")
            ));
        }
        let text = match member {
            "acquisition_type" => {
                let filled = self.term(terms, alias, "acquisition_type_filled", at)?.sql;
                let read = self.term(terms, alias, "mr_acquisition_type", at)?.sql;
                format!("COALESCE({filled}, {read}, '')")
            }
            "orientation" => format!(
                "COALESCE({}, '')",
                self.term(terms, alias, "orientation", at)?.sql
            ),
            physics => {
                let x = self.term(terms, alias, physics, at)?.sql;
                let integer = match step {
                    Some(s) => {
                        let scaled = format!("({x} / {s})");
                        if self.sql.is_pg() {
                            format!("CAST(round(({scaled})::numeric) AS BIGINT)")
                        } else {
                            format!("CAST(round({scaled}) AS INTEGER)")
                        }
                    }
                    None => self.sql.rounded_key(&x, 3),
                };
                format!("COALESCE(CAST({integer} AS TEXT), '')")
            }
        };
        Ok(text)
    }

    fn expr(&mut self, c: &Clause, terms: &[(String, Term)], alias: &str, at: &str) -> R<Term> {
        let op = c.op.as_str();
        let strict = c
            .opts
            .get("strict")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let plain = |s: String| Ok(Term::plain(s));
        let dot = |s: &str| {
            if alias.is_empty() {
                s.to_string()
            } else {
                format!("{alias}.{s}")
            }
        };
        match op {
            "field" => {
                let p = c
                    .ref_name()
                    .ok_or_else(|| err(at, "a field ref names a path"))?;
                self.term(terms, alias, p, at)
            }
            "param" => {
                let name = c
                    .ref_name()
                    .ok_or_else(|| err(at, "a param ref names a parameter"))?;
                self.param_term(name, at)
            }
            "axis" => Err(err(
                at,
                "an axis is read through has or =, which compile it as a predicate",
            )),
            "derived" => self.derived(c, terms, alias, at),
            "=" | "<>" | ">" | ">=" | "<" | "<=" => {
                if let Some(Arg::Clause(l)) = c.args.first()
                    && l.op == "axis"
                {
                    let axis = l.ref_name().unwrap_or("");
                    let value = c
                        .args
                        .get(1)
                        .ok_or_else(|| err(at, "an axis comparison needs a value"))?;
                    let ax = self.p(Param::from(axis), Type::Text);
                    let v = self.arg(value, terms, alias, at)?;
                    let exists = format!(
                        "EXISTS (SELECT 1 FROM {} ax WHERE ax.stack_id = {} AND ax.axis = {ax} AND ax.value = {})",
                        self.q("classification_axis"),
                        dot("k"),
                        v.sql
                    );
                    return plain(if op == "<>" {
                        format!("NOT {exists}")
                    } else {
                        exists
                    });
                }
                let a = self.arg(
                    c.args
                        .first()
                        .ok_or_else(|| err(at, format!("{op} needs two arguments")))?,
                    terms,
                    alias,
                    at,
                )?;
                let b = self.arg(
                    c.args
                        .get(1)
                        .ok_or_else(|| err(at, format!("{op} needs two arguments")))?,
                    terms,
                    alias,
                    at,
                )?;
                plain(self.compare(op, a, b, strict))
            }
            "~=" => {
                let a = self.arg(
                    c.args
                        .first()
                        .ok_or_else(|| err(at, "~= needs two arguments"))?,
                    terms,
                    alias,
                    at,
                )?;
                let b = self.arg(
                    c.args
                        .get(1)
                        .ok_or_else(|| err(at, "~= needs two arguments"))?,
                    terms,
                    alias,
                    at,
                )?;
                let tol = match c.opts.get("tol") {
                    Some(Value::Number(n)) => {
                        self.p(Param::Double(n.as_f64().unwrap_or(0.0)), Type::Double)
                    }
                    Some(v) => {
                        let inner = crate::ast::clause_of(v).map_err(|m| err(at, m))?;
                        self.expr(&inner, terms, alias, at)?.sql
                    }
                    None => return Err(err(at, "~= needs tol")),
                };
                plain(format!("(ABS({} - {}) <= {tol})", a.sql, b.sql))
            }
            "in" | "not_in" => {
                let a = self.arg(
                    c.args
                        .first()
                        .ok_or_else(|| err(at, "in needs a value and a list"))?,
                    terms,
                    alias,
                    at,
                )?;
                let list = self.list_placeholders(
                    c.args.get(1).ok_or_else(|| err(at, "in needs a list"))?,
                    at,
                )?;
                plain(format!(
                    "({} {}IN {list})",
                    a.sql,
                    if op == "not_in" { "NOT " } else { "" }
                ))
            }
            "has" => {
                let Some(Arg::Clause(l)) = c.args.first() else {
                    return Err(err(at, "has takes an axis and a value"));
                };
                if l.op != "axis" {
                    return Err(err(at, "has takes an axis"));
                }
                let axis = l.ref_name().unwrap_or("");
                let ax = self.p(Param::from(axis), Type::Text);
                let v = self.arg(
                    c.args.get(1).ok_or_else(|| err(at, "has needs a value"))?,
                    terms,
                    alias,
                    at,
                )?;
                plain(format!(
                    "EXISTS (SELECT 1 FROM {} ax WHERE ax.stack_id = {} AND ax.axis = {ax} AND ax.value = {})",
                    self.q("classification_axis"),
                    dot("k"),
                    v.sql
                ))
            }
            "picked" => {
                let role = self
                    .opt_placeholder(&c.opts, "role", at)?
                    .ok_or_else(|| err(at, "picked names its role: {role}"))?;
                let digest = self.p(Param::from(self.ctx.scheme_digest.as_str()), Type::Text);
                let mut sql = format!(
                    "EXISTS (SELECT 1 FROM {} p JOIN {} ps ON ps.pick_id = p.id WHERE ps.stack_id = {} AND p.role = {role} \
                     AND p.withdrawn_at IS NULL AND p.scheme_digest = {digest}",
                    self.q("pick"),
                    self.q("pick_stack"),
                    dot("k")
                );
                if let Some(model) = self.opt_placeholder(&c.opts, "model", at)? {
                    sql.push_str(&format!(" AND p.model = {model}"));
                }
                sql.push(')');
                plain(sql)
            }
            "not_null" | "is_null" => {
                let a = self.arg(
                    c.args
                        .first()
                        .ok_or_else(|| err(at, format!("{op} needs an argument")))?,
                    terms,
                    alias,
                    at,
                )?;
                plain(format!(
                    "({} IS {}NULL)",
                    a.sql,
                    if op == "not_null" { "NOT " } else { "" }
                ))
            }
            "contains" | "starts_with" => {
                let a = self.arg(
                    c.args
                        .first()
                        .ok_or_else(|| err(at, format!("{op} needs a field and a text")))?,
                    terms,
                    alias,
                    at,
                )?;
                let Some(Arg::Text(needle)) = c.args.get(1) else {
                    return Err(err(at, format!("{op} takes a literal text")));
                };
                let pattern = match op {
                    "contains" => format!("%{}%", like_escape(&needle.to_lowercase())),
                    _ => format!("{}%", like_escape(&needle.to_lowercase())),
                };
                let ph = self.p(Param::from(pattern), Type::Text);
                let column = a.ci.clone().unwrap_or_else(|| format!("LOWER({})", a.sql));
                plain(self.sql.like(&column, &ph))
            }
            "and" | "or" => {
                let mut parts = Vec::new();
                for a in &c.args {
                    parts.push(self.arg(a, terms, alias, at)?.sql);
                }
                plain(format!(
                    "({})",
                    parts.join(if op == "and" { " AND " } else { " OR " })
                ))
            }
            "not" => {
                let a = self.arg(
                    c.args
                        .first()
                        .ok_or_else(|| err(at, "not needs an argument"))?,
                    terms,
                    alias,
                    at,
                )?;
                plain(format!("(NOT {})", a.sql))
            }
            "+" | "-" | "*" | "/" => {
                let a = self.arg(
                    c.args
                        .first()
                        .ok_or_else(|| err(at, format!("{op} needs two arguments")))?,
                    terms,
                    alias,
                    at,
                )?;
                let b = self.arg(
                    c.args
                        .get(1)
                        .ok_or_else(|| err(at, format!("{op} needs two arguments")))?,
                    terms,
                    alias,
                    at,
                )?;
                if op == "/" {
                    return plain(format!(
                        "({} / {})",
                        self.sql.as_double(&a.sql),
                        self.sql.as_double(&b.sql)
                    ));
                }
                plain(format!("({} {op} {})", a.sql, b.sql))
            }
            "abs" => {
                let a = self.arg(
                    c.args
                        .first()
                        .ok_or_else(|| err(at, "abs needs an argument"))?,
                    terms,
                    alias,
                    at,
                )?;
                plain(format!("ABS({})", a.sql))
            }
            "round" => {
                let a = self.arg(
                    c.args
                        .first()
                        .ok_or_else(|| err(at, "round needs an argument"))?,
                    terms,
                    alias,
                    at,
                )?;
                let places = match c.args.get(1) {
                    Some(Arg::Int(n)) => *n as u32,
                    None => 0,
                    _ => return Err(err(at, "round takes a literal number of places")),
                };
                if c.opts.get("key").and_then(Value::as_bool).unwrap_or(false) {
                    plain(self.sql.rounded_key(&a.sql, places))
                } else {
                    plain(self.sql.rounded(&a.sql, places))
                }
            }
            "coalesce" => {
                let mut parts = Vec::new();
                for a in &c.args {
                    parts.push(self.arg(a, terms, alias, at)?.sql);
                }
                plain(format!("COALESCE({})", parts.join(", ")))
            }
            "case" => {
                let cond = self.arg(
                    c.args
                        .first()
                        .ok_or_else(|| err(at, "case takes a condition, a then and an else"))?,
                    terms,
                    alias,
                    at,
                )?;
                let then = self.arg(
                    c.args.get(1).ok_or_else(|| err(at, "case takes a then"))?,
                    terms,
                    alias,
                    at,
                )?;
                let otherwise = match c.args.get(2) {
                    Some(e) => self.arg(e, terms, alias, at)?.sql,
                    None => "NULL".into(),
                };
                plain(format!(
                    "CASE WHEN {} THEN {} ELSE {otherwise} END",
                    cond.sql, then.sql
                ))
            }
            "concat" => {
                let mut parts = Vec::new();
                for a in &c.args {
                    parts.push(self.arg(a, terms, alias, at)?.sql);
                }
                plain(format!("({})", parts.join(" || ")))
            }
            "days_between" => {
                let a = self.arg(
                    c.args
                        .first()
                        .ok_or_else(|| err(at, "days_between needs two dates"))?,
                    terms,
                    alias,
                    at,
                )?;
                let b = self.arg(
                    c.args
                        .get(1)
                        .ok_or_else(|| err(at, "days_between needs two dates"))?,
                    terms,
                    alias,
                    at,
                )?;
                plain(self.sql.days_between(&a.sql, &b.sql))
            }
            "shift" => {
                let d = self.arg(
                    c.args
                        .first()
                        .ok_or_else(|| err(at, "shift needs a date and a count"))?,
                    terms,
                    alias,
                    at,
                )?;
                let n = self.arg(
                    c.args
                        .get(1)
                        .ok_or_else(|| err(at, "shift needs a count"))?,
                    terms,
                    alias,
                    at,
                )?;
                let unit = c.opts.get("unit").and_then(Value::as_str).unwrap_or("day");
                let per = match unit {
                    "day" => 1,
                    "month" => 31,
                    "year" => 366,
                    other => return Err(err(at, format!("{other} is not a unit"))),
                };
                let days = if per == 1 {
                    n.sql
                } else {
                    format!("({} * {per})", n.sql)
                };
                plain(self.sql.shift_days(&d.sql, &days))
            }
            "age_at" => {
                let birth = self.arg(
                    c.args
                        .first()
                        .ok_or_else(|| err(at, "age_at needs a birth date and a day"))?,
                    terms,
                    alias,
                    at,
                )?;
                let day = self.arg(
                    c.args.get(1).ok_or_else(|| err(at, "age_at needs a day"))?,
                    terms,
                    alias,
                    at,
                )?;
                let birth2 = self.again(&birth);
                let day2 = self.again(&day);
                plain(self.sql.age_at((&birth.sql, &birth2), (&day.sql, &day2)))
            }
            "bucket" | "part" => {
                let d = self.arg(
                    c.args
                        .first()
                        .ok_or_else(|| err(at, format!("{op} needs a date")))?,
                    terms,
                    alias,
                    at,
                )?;
                let unit = c
                    .opts
                    .get("unit")
                    .and_then(Value::as_str)
                    .ok_or_else(|| err(at, format!("{op} needs {{unit}}")))?;
                plain(if op == "bucket" {
                    self.sql.bucket(&d.sql, unit)?
                } else {
                    self.sql.part(&d.sql, unit)?
                })
            }
            "least" | "greatest" => {
                let a = self.arg(
                    c.args
                        .first()
                        .ok_or_else(|| err(at, format!("{op} needs two arguments")))?,
                    terms,
                    alias,
                    at,
                )?;
                let b = self.arg(
                    c.args
                        .get(1)
                        .ok_or_else(|| err(at, format!("{op} needs two arguments")))?,
                    terms,
                    alias,
                    at,
                )?;
                plain(if op == "least" {
                    self.sql.least(&a.sql, &b.sql)
                } else {
                    self.sql.greatest(&a.sql, &b.sql)
                })
            }
            "json" => {
                let a = self.arg(
                    c.args
                        .first()
                        .ok_or_else(|| err(at, "json needs a text and a key"))?,
                    terms,
                    alias,
                    at,
                )?;
                let Some(Arg::Text(key)) = c.args.get(1) else {
                    return Err(err(at, "json takes a literal key"));
                };
                if !key
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                {
                    return Err(err(at, "a json key is letters, digits and underscores"));
                }
                plain(self.sql.json_text(&a.sql, key))
            }
            "ordinal" | "prev" | "next" => {
                // the interval's first day, then precision (finer first), then the key
                let finer = match terms
                    .iter()
                    .find(|(n, _)| n == "day")
                    .and_then(|(_, t)| t.prec.clone())
                {
                    Some(p) => format!(
                        ", CASE {} WHEN 'day' THEN 0 WHEN 'month' THEN 1 ELSE 2 END",
                        dot(&p)
                    ),
                    None => String::new(),
                };
                let seq = format!(
                    "PARTITION BY {} ORDER BY {}{finer}, {}",
                    dot("subj"),
                    dot("day"),
                    dot("k")
                );
                if op == "ordinal" {
                    return plain(format!("ROW_NUMBER() OVER ({seq})"));
                }
                let a = self.arg(
                    c.args
                        .first()
                        .ok_or_else(|| err(at, format!("{op} needs a clause")))?,
                    terms,
                    alias,
                    at,
                )?;
                let f = if op == "prev" { "LAG" } else { "LEAD" };
                Ok(Term {
                    sql: format!("{f}({}) OVER ({seq})", a.sql),
                    prec: None,
                    ci: None,
                    param: None,
                })
            }
            "share" => {
                let of = c
                    .opts
                    .get("of")
                    .and_then(Value::as_str)
                    .ok_or_else(|| err(at, "share names {of, over}"))?;
                let over = c
                    .opts
                    .get("over")
                    .and_then(Value::as_str)
                    .ok_or_else(|| err(at, "share names {of, over}"))?;
                let num = self.term(terms, alias, of, at)?.sql;
                let over_frame = self.frames.get(over).cloned().unwrap_or_default();
                let counted = if over_frame.has_subj {
                    "COUNT(DISTINCT o.subj)"
                } else {
                    "COUNT(*)"
                };
                let denom = format!("(SELECT {counted} FROM {} o)", cte_name(over));
                plain(format!(
                    "({} / {})",
                    self.sql.as_double(&num),
                    self.sql.as_double(&denom)
                ))
            }
            "change" => Err(err(
                at,
                "change is a binding of a subject set, not an expression",
            )),
            op if AGGREGATES.contains(&op) => {
                let target = c
                    .opts
                    .get("set")
                    .and_then(Value::as_str)
                    .ok_or_else(|| err(at, format!("{op} needs {{set}}")))?
                    .to_string();
                let child = self.frames.get(&target).cloned().unwrap_or_default();
                let child_set = self
                    .ask
                    .sets
                    .get(&target)
                    .ok_or_else(|| err(at, format!("no set {target}")))?;
                let grain = self.current;
                let link = if child_set.grain == Grain::Group {
                    let key_path = format!("{}.id", grain.map(|g| g.name()).unwrap_or("subject"));
                    let (_, gcol) = child
                        .group_by
                        .iter()
                        .find(|(p, _)| p == &key_path)
                        .ok_or_else(|| {
                            err(at, format!("group {target} is not keyed by this grain"))
                        })?;
                    format!("ch.{gcol} = {}", dot("k"))
                } else if Some(child_set.grain) == grain {
                    format!("ch.k = {}", dot("k"))
                } else {
                    let g = grain.ok_or_else(|| err(at, "an aggregate needs a grain"))?;
                    format!("{} = {}", Self::link(&child, g, "ch", at)?, dot("k"))
                };
                let inner = match c.args.first() {
                    Some(Arg::Clause(inner)) => Some(self.expr(inner, &child.terms, "ch", at)?.sql),
                    Some(_) => return Err(err(at, "an aggregate's argument is a clause")),
                    None => None,
                };
                let agg = self.aggregate(op, inner.as_deref(), at)?;
                plain(format!(
                    "(SELECT {agg} FROM {} ch WHERE {link})",
                    cte_name(&target)
                ))
            }
            other => Err(err(at, format!("{other} is not an op of the language"))),
        }
    }

    /// The final SELECT over the answer's set.
    fn answer(&mut self) -> R<String> {
        let out = &self.ask.out;
        let frame = self.frames.get(&out.set).cloned().unwrap_or_default();
        let terms = frame.terms.clone();
        let mut cols = vec!["o.k AS _key".to_string(), "o.subj AS _subject".to_string()];
        let mut names = vec!["_key".to_string(), "_subject".to_string()];
        let mut code_columns = Vec::new();
        match out.level {
            crate::ast::Level::Count => {
                let subjects = if frame.has_subj {
                    "COUNT(DISTINCT o.subj)"
                } else {
                    "COUNT(*)"
                };
                let sql = format!(
                    "SELECT {} AS rows_, {} AS subjects_ FROM {} o",
                    self.sql.as_bigint("COUNT(*)"),
                    self.sql.as_bigint(subjects),
                    cte_name(&out.set)
                );
                self.answer_columns = vec!["rows".into(), "subjects".into()];
                return Ok(sql);
            }
            crate::ast::Level::Boolean => {
                let sql = format!(
                    "SELECT CASE WHEN EXISTS (SELECT 1 FROM {} o) THEN 1 ELSE 0 END AS any_",
                    cte_name(&out.set)
                );
                self.answer_columns = vec!["any".into()];
                return Ok(sql);
            }
            _ => {}
        }
        for (i, c) in out.columns.iter().enumerate() {
            let e = self.expr(c, &terms, "o", &format!("out.columns[{i}]"))?;
            let name = c
                .ref_name()
                .map(str::to_string)
                .unwrap_or_else(|| format!("col{i}"));
            let is_date = e.prec.is_some()
                || name == "birth_date"
                || name.ends_with("date")
                || name == "first"
                || name == "last"
                || name.ends_with(".first")
                || name.ends_with(".last");
            let rendered = if is_date {
                format!("CAST({} AS TEXT)", e.sql)
            } else {
                e.sql
            };
            if name == "code" || name == "subject.code" {
                code_columns.push(cols.len());
            }
            cols.push(format!("{rendered} AS c{i}"));
            names.push(name);
        }
        let mut ord_cols = Vec::new();
        let mut order = Vec::new();
        for (i, o) in out.order.iter().enumerate() {
            let e = self.expr(&o.0, &terms, "o", &format!("out.order[{i}]"))?;
            ord_cols.push(format!("{} AS ord_{i}", e.sql));
            order.push(self.sql.order_term(&format!("z.ord_{i}"), o.1));
        }
        order.push("z._key ASC".into());
        let mut inner_cols = cols.clone();
        inner_cols.extend(ord_cols);
        let mut inner = format!(
            "SELECT {} FROM {} o",
            inner_cols.join(", "),
            cte_name(&out.set)
        );
        if let Some(after) = self.ctx.after {
            let ph = self.p(Param::Int(after), Type::Int);
            inner.push_str(&format!(" WHERE o.k > {ph}"));
        }
        let outer_cols: Vec<String> = (0..cols.len())
            .map(|i| match i {
                0 => "z._key".to_string(),
                1 => "z._subject".to_string(),
                n => format!("z.c{}", n - 2),
            })
            .collect();
        let mut sql = format!(
            "SELECT {} FROM ({inner}) z ORDER BY {}",
            outer_cols.join(", "),
            order.join(", ")
        );
        if let Some(limit) = out.limit.or(self.ctx.limit) {
            sql.push_str(&format!(" LIMIT {limit}"));
        }
        self.answer_columns = names;
        self.code_columns = code_columns;
        Ok(sql)
    }
}

/// Compile a desugared, validated ask.
pub fn compile(ask: &Ask, validated: &Validated, ctx: &Context<'_>) -> R<Compiled> {
    let mut reads: HashMap<String, usize> = HashMap::new();
    for set in ask.sets.values() {
        for r in set.reads() {
            *reads.entry(r.to_string()).or_insert(0) += 1;
        }
    }
    *reads.entry(ask.out.set.clone()).or_insert(0) += 1;
    let mut b = Builder {
        ctx,
        ask,
        sql: Sql {
            dialect: ctx.dialect,
        },
        params: Vec::new(),
        ctes: Vec::new(),
        frames: BTreeMap::new(),
        reads,
        current: None,
        external: external_paths(ask),
        answer_columns: Vec::new(),
        code_columns: Vec::new(),
    };
    for name in &validated.order {
        b.current = Some(ask.sets[name].grain);
        b.build_set(name)?;
    }
    b.current = ask.sets.get(&ask.out.set).map(|s| s.grain);
    let final_sql = b.answer()?;
    let sql = format!("WITH {} {final_sql}", b.ctes.join(", "));
    Ok(Compiled {
        sql,
        params: b.params,
        columns: b.answer_columns.clone(),
        code_columns: b.code_columns.clone(),
    })
}
