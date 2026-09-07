// SPDX-License-Identifier: AGPL-3.0-only

//! The compiler (§11): one statement per ask, one SQL text per backend,
//! sets in topological order as CTEs, each set a layered subselect in the
//! order of rule 5. The eleven hooks and the four closures live in
//! [`Sql`], the only place the two dialects differ; everything above it is
//! the same text.
//!
//! What lands here (slice 5): the base relation of every grain, `from` (a
//! set, a role, a handle, an uploaded list), `of`, `algebra`, `group` with
//! its aggregates, `has`, `bind`, `where`, `pick` with its ties, the answer
//! with its columns, order, keyset paging and limit, and the comparisons
//! against a coarse date (§5.3). `near`, `attach`, `same`, `change`,
//! `share`, the sequences and the derived fields are slice 6.

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

/// What a set's CTE projects, by name, and how to reach it from a reader.
#[derive(Debug, Clone, Default)]
struct Frame {
    /// Bindings, by name, as their column in the CTE.
    bindings: Vec<(String, String)>,
    /// A group's by paths, as their columns.
    group_by: Vec<(String, String)>,
    /// Whether the CTE carries `subj`, `day`, `prec`.
    has_subj: bool,
    has_day: bool,
    has_prec: bool,
    /// The `of` ancestor set, for `<of>.<binding>` paths.
    of: Option<String>,
    /// Whether the set was picked, for `pick.*`.
    picked: bool,
}

fn column_name(binding: &str) -> String {
    format!("b_{}", binding.replace('.', "__"))
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
    /// The field paths other sets read through an aggregate or a group
    /// over each set, which that set must project.
    external: HashMap<String, BTreeSet<String>>,
    answer_columns: Vec<String>,
    code_columns: Vec<usize>,
}

impl<'a> Builder<'a> {
    fn q(&self, table: &str) -> String {
        qualified(self.ctx.schema.as_deref(), table)
    }

    /// Bind one parameter and return its placeholder. On Postgres an
    /// integer or a double is cast explicitly: the driver sends an int8 and
    /// a placeholder beside an int4 expression would infer int4.
    fn p(&mut self, v: Param, ty: Type) -> String {
        self.params.push(v);
        let n = self.params.len();
        match (self.ctx.dialect, ty) {
            (Dialect::Postgres, Type::Int) => format!("${n}::bigint"),
            (Dialect::Postgres, Type::Double) => format!("${n}::double precision"),
            (Dialect::Postgres, Type::Text) => format!("${n}::text"),
            (d, t) => d.param(n, t),
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
}

/// The set of field paths a set's clauses read, to project them in its
/// first layer.
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

fn set_paths(set: &Set, out_columns: &[Clause]) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    for (_, c) in &set.bind.0 {
        paths_in(c, &mut paths);
    }
    for c in &set.where_ {
        paths_in(c, &mut paths);
    }
    if let Some(pk) = &set.pick {
        for o in &pk.by {
            paths_in(&o.0, &mut paths);
        }
    }
    for c in out_columns {
        paths_in(c, &mut paths);
    }
    paths
}

/// The name a projected field gets in the layers.
fn field_col(path: &str) -> String {
    format!("f_{}", path.replace('.', "__"))
}

impl<'a> Builder<'a> {
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

    /// One set as a CTE.
    fn build_set(&mut self, name: &str, out_columns: &[Clause], out_order: &[Clause]) -> R<()> {
        let set = &self.ask.sets[name];
        let path = format!("sets.{name}");
        if set.grain == Grain::Group {
            return self.build_group(name);
        }
        if let Some(a) = &set.algebra {
            return self.build_algebra(name, a);
        }
        if !set.near.is_empty() {
            return Err(err(format!("{path}.near"), "near lands with slice 6"));
        }
        if !set.attach.is_empty() {
            return Err(err(format!("{path}.attach"), "attach lands with slice 6"));
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
        // what the first layer projects
        let mut projected: Vec<String> = vec![
            format!("{} AS k", base.key),
            format!(
                "{} AS subj",
                base.subj.clone().unwrap_or_else(|| "NULL".into())
            ),
            format!(
                "{} AS day",
                base.day.clone().unwrap_or_else(|| "NULL".into())
            ),
            format!(
                "{} AS prec",
                base.prec.clone().unwrap_or_else(|| "NULL".into())
            ),
            format!(
                "{} AS session_k",
                base.session_k.clone().unwrap_or_else(|| "NULL".into())
            ),
            format!(
                "{} AS study_k",
                base.study_k.clone().unwrap_or_else(|| "NULL".into())
            ),
            format!(
                "{} AS series_k",
                base.series_k.clone().unwrap_or_else(|| "NULL".into())
            ),
            format!(
                "{} AS stack_k",
                base.stack_k.clone().unwrap_or_else(|| "NULL".into())
            ),
        ];
        // a source: its bindings come along
        match &set.from {
            None => {}
            Some(Src::Set(s)) => {
                from.push_str(&format!(" JOIN {} x ON x.k = {}", cte_name(s), base.key));
                let src = self.frames.get(s).cloned().unwrap_or_default();
                for (b, col) in &src.bindings {
                    projected.push(format!("x.{col} AS {col}"));
                    frame.bindings.push((b.clone(), col.clone()));
                }
                frame.of.clone_from(&src.of);
                frame.picked = src.picked;
                if src.picked {
                    for extra in ["pick_tied", "pick_candidates", "pick_rank"] {
                        projected.push(format!("x.{extra} AS {extra}"));
                    }
                }
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
                (Grain::Cohort, Grain::Subject) => {
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
                (Grain::Cohort, _) => {
                    from.push_str(&format!(
                        " JOIN {} cm ON cm.subject_id = su.id AND cm.left_at IS NULL",
                        self.q("cohort_member")
                    ));
                    "a.k = cm.cohort_id".to_string()
                }
                (g, h) => {
                    return Err(err(
                        format!("{path}.of"),
                        format!("{g} is not an ancestor of {h}"),
                    ));
                }
            };
            from.push_str(&format!(" JOIN {} a ON {on}", cte_name(of)));
            let anc = self.frames.get(of).cloned().unwrap_or_default();
            for (b, col) in &anc.bindings {
                let mine = format!("o_{}", &col[2..]);
                projected.push(format!("a.{col} AS {mine}"));
                frame.bindings.push((format!("{of}.{b}"), mine));
            }
            if ancestor.grain == Grain::Cohort {
                projected.push("a.k AS cohort_k".into());
            }
            frame.of = Some(of.clone());
        }
        // every field the set reads, projected once
        let mut wanted = set_paths(
            set,
            if name == self.ask.out.set {
                out_columns
            } else {
                &[]
            },
        );
        if let Some(ext) = self.external.get(name) {
            wanted.extend(ext.iter().cloned());
        }
        if name == self.ask.out.set {
            for c in out_order {
                paths_in(c, &mut wanted);
            }
        }
        let mut field_cols: Vec<(String, Term)> = Vec::new();
        for p in &wanted {
            if frame.bindings.iter().any(|(b, _)| b == p) {
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
                field_cols.push((p.clone(), layered));
            }
        }
        let mut layer = format!("SELECT {} FROM {from}", projected.join(", "));
        if !wheres.is_empty() {
            layer.push_str(&format!(" WHERE {}", wheres.join(" AND ")));
        }
        wheres.clear();

        // the terms a clause may read in the layers
        let mut terms: Vec<(String, Term)> = field_cols;
        for (b, col) in &frame.bindings {
            terms.push((b.clone(), Term::plain(col.clone())));
        }
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
        if frame.picked {
            terms.push(("pick.tied".into(), Term::plain("pick_tied".into())));
            terms.push((
                "pick.candidates".into(),
                Term::plain("pick_candidates".into()),
            ));
            terms.push(("pick.rank".into(), Term::plain("pick_rank".into())));
        }

        // has, before bind: a count becomes a binding, a bound count a predicate
        let mut has_preds: Vec<String> = Vec::new();
        for (i, h) in set.has.iter().enumerate() {
            let hp = format!("{path}.has[{i}]");
            let child = self.frames.get(&h.set).cloned().unwrap_or_default();
            let child_set = &self.ask.sets[&h.set];
            let count = if child_set.grain == Grain::Group {
                let key_path = format!("{}.id", set.grain.name());
                let (_, gcol) = child
                    .group_by
                    .iter()
                    .find(|(p, _)| p == &key_path)
                    .ok_or_else(|| err(hp.clone(), "the group is not keyed by this grain"))?;
                format!(
                    "(SELECT COUNT(*) FROM {} ch WHERE ch.{gcol} = q.k)",
                    cte_name(&h.set)
                )
            } else if child_set.grain == set.grain {
                format!(
                    "(SELECT COUNT(*) FROM {} ch WHERE ch.k = q.k)",
                    cte_name(&h.set)
                )
            } else {
                let link = Self::link(&child, set.grain, "ch", &hp)?;
                let mut inner = format!(
                    "SELECT COUNT(*) FROM {} ch WHERE {link} = q.k",
                    cte_name(&h.set)
                );
                if let Some(w) = &h.window {
                    let (lo, hi) = window_days(w, &hp)?;
                    let anchor = match &h.on {
                        Some(on) => terms
                            .iter()
                            .find(|(n, _)| n == on)
                            .map(|(_, t)| t.sql.clone())
                            .ok_or_else(|| err(hp.clone(), format!("{on} is not bound")))?,
                        None => "q.day".to_string(),
                    };
                    let delta = self.sql.days_between("ch.day", &format!("({anchor})"));
                    if let Some(lo) = lo {
                        inner.push_str(&format!(" AND {delta} >= {lo}"));
                    }
                    if let Some(hi) = hi {
                        inner.push_str(&format!(" AND {delta} <= {hi}"));
                    }
                }
                format!("({inner})")
            };
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
        }
        self.frames.insert(name.to_string(), frame);
        self.push_cte(name, &layer);
        Ok(())
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
            // the ancestor's field, reachable through the base
            let ancestor = self.ask.sets.get(of)?;
            let level = ancestor.grain.name();
            if ancestor.grain == Grain::Cohort {
                return (rest == "id").then(|| Term::plain("a.k".into()));
            }
            return self.base_column(level, rest);
        }
        self.base_column(grain.name(), path)
    }

    fn build_group(&mut self, name: &str) -> R<()> {
        let set = &self.ask.sets[name];
        let path = format!("sets.{name}");
        let g = set
            .group
            .as_ref()
            .ok_or_else(|| err(&path, "a group set needs group"))?;
        let child = self.frames.get(&g.of).cloned().unwrap_or_default();
        let child_set = &self.ask.sets[&g.of];
        // the child's terms, as its CTE exposes them
        let child_terms = self.cte_terms(&g.of, &child, child_set, &[]);
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
        // aggregates and arithmetic over them, in order
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
        // arithmetic over the aggregates, in layers above the GROUP BY
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
        self.frames.insert(name.to_string(), frame);
        self.push_cte(name, &layer);
        Ok(())
    }

    /// What a set's CTE exposes to a reader by alias: its bindings, its
    /// day, and the fields it projected.
    fn cte_terms(
        &self,
        name: &str,
        frame: &Frame,
        set: &Set,
        extra: &[Clause],
    ) -> Vec<(String, Term)> {
        let mut out: Vec<(String, Term)> = Vec::new();
        for (b, col) in &frame.bindings {
            out.push((b.clone(), Term::plain(col.clone())));
        }
        for (label, col) in &frame.group_by {
            out.push((label.clone(), Term::plain(col.clone())));
        }
        if frame.picked {
            out.push(("pick.tied".into(), Term::plain("pick_tied".into())));
            out.push((
                "pick.candidates".into(),
                Term::plain("pick_candidates".into()),
            ));
            out.push(("pick.rank".into(), Term::plain("pick_rank".into())));
        }
        if frame.has_day {
            out.push((
                "day".into(),
                Term {
                    sql: "day".into(),
                    prec: frame.has_prec.then(|| "prec".to_string()),
                    ci: None,
                    param: None,
                },
            ));
        }
        let mut wanted = set_paths(set, extra);
        for c in extra {
            paths_in(c, &mut wanted);
        }
        if let Some(ext) = self.external.get(name) {
            wanted.extend(ext.iter().cloned());
        }
        for p in wanted {
            if !out.iter().any(|(n, _)| n == &p)
                && let Some(t) = self.field_term(set, frame, &p)
            {
                let col = field_col(&p);
                out.push((
                    p.clone(),
                    Term {
                        sql: col.clone(),
                        prec: t.prec.map(|_| format!("{col}__prec")),
                        ci: t.ci.map(|_| format!("{col}__ci")),
                        param: None,
                    },
                ));
            }
        }
        // the child's own key and subject, for a by on them
        out.push((format!("{}.id", set.grain.name()), Term::plain("k".into())));
        if frame.has_subj {
            out.push(("subject.id".into(), Term::plain("subj".into())));
        }
        if let Some(of) = &frame.of
            && let Some(anc) = self.ask.sets.get(of)
            && anc.grain == Grain::Cohort
        {
            out.push(("cohort.id".into(), Term::plain("cohort_k".into())));
        }
        out
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
        let base_cols = [
            "k",
            "subj",
            "day",
            "prec",
            "session_k",
            "study_k",
            "series_k",
            "stack_k",
        ];
        let layer = match a.op {
            AlgOp::Union => {
                let mut common: Vec<(String, String)> = lf.bindings.clone();
                for other in a.sets.iter().skip(1) {
                    let of = self.frames.get(other).cloned().unwrap_or_default();
                    common.retain(|(b, _)| of.bindings.iter().any(|(ob, _)| ob == b));
                }
                frame.bindings = common.clone();
                frame.picked = false;
                let mut parts = Vec::new();
                for s in &a.sets {
                    let mut cols: Vec<String> =
                        base_cols.iter().map(|c| format!("l.{c}")).collect();
                    for (_, col) in &common {
                        cols.push(format!("l.{col}"));
                    }
                    if let Some(t) = &a.tag {
                        let _ = t;
                        cols.push(format!("'{s}' AS tag"));
                    }
                    parts.push(format!("SELECT {} FROM {} l", cols.join(", "), cte_name(s)));
                }
                if let Some(t) = &a.tag {
                    frame.bindings.push((t.clone(), "tag".into()));
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
        Ok(Term {
            sql: format!("{alias}.{}", t.sql),
            prec: t.prec.map(|p| format!("{alias}.{p}")),
            ci: t.ci.map(|c| format!("{alias}.{c}")),
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

    /// A term used once more: a parameter is bound again, anything else is
    /// its text.
    fn again(&mut self, t: &Term) -> String {
        match &t.param {
            Some((v, ty)) => self.p(v.clone(), *ty),
            None => t.sql.clone(),
        }
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

    /// A comparison, coarse dates included (§5.3): forgiving by default,
    /// the certain reading under `strict: true`.
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

    fn expr(&mut self, c: &Clause, terms: &[(String, Term)], alias: &str, at: &str) -> R<Term> {
        let op = c.op.as_str();
        let strict = c
            .opts
            .get("strict")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let plain = |s: String| Ok(Term::plain(s));
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
            "derived" => Err(err(at, "derived fields land with slice 6")),
            "=" | "<>" | ">" | ">=" | "<" | "<=" => {
                // an axis predicate: EXISTS on the axis rows
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
                        "EXISTS (SELECT 1 FROM {} ax WHERE ax.stack_id = {alias}.k AND ax.axis = {ax} AND ax.value = {})",
                        self.q("classification_axis"),
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
                // a multi-valued axis holds a value
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
                    "EXISTS (SELECT 1 FROM {} ax WHERE ax.stack_id = {alias}.k AND ax.axis = {ax} AND ax.value = {})",
                    self.q("classification_axis"),
                    v.sql
                ))
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
                    // a Double cast so SQLite never divides integers
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
                // ["case", {}, cond, then, else]
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
            op if AGGREGATES.contains(&op) => {
                // a correlated aggregate over a descendant or a group
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
                let grain = self.frame_grain(alias, terms);
                let link = if child_set.grain == Grain::Group {
                    let key_path = format!("{}.id", grain.map(|g| g.name()).unwrap_or("subject"));
                    let (_, gcol) = child
                        .group_by
                        .iter()
                        .find(|(p, _)| p == &key_path)
                        .ok_or_else(|| {
                            err(at, format!("group {target} is not keyed by this grain"))
                        })?;
                    format!("ch.{gcol} = {alias}.k")
                } else if Some(child_set.grain) == grain {
                    format!("ch.k = {alias}.k")
                } else {
                    let g = grain.ok_or_else(|| err(at, "an aggregate needs a grain"))?;
                    format!("{} = {alias}.k", Self::link(&child, g, "ch", at)?)
                };
                let child_terms = self.cte_terms(&target, &child, child_set, &[]);
                let inner = match c.args.first() {
                    Some(Arg::Clause(inner)) => Some(self.expr(inner, &child_terms, "ch", at)?.sql),
                    Some(_) => return Err(err(at, "an aggregate's argument is a clause")),
                    None => None,
                };
                let agg = self.aggregate(op, inner.as_deref(), at)?;
                plain(format!(
                    "(SELECT {agg} FROM {} ch WHERE {link})",
                    cte_name(&target)
                ))
            }
            "ordinal" | "prev" | "next" | "change" | "share" => {
                Err(err(at, format!("{op} lands with slice 6")))
            }
            other => Err(err(at, format!("{other} is not an op of the language"))),
        }
    }

    /// The grain of the set being built.
    fn frame_grain(&self, _alias: &str, _terms: &[(String, Term)]) -> Option<Grain> {
        self.current
    }
}

const AGGREGATES: &[&str] = &["count", "distinct", "min", "max", "sum", "avg", "list"];

/// For every set, the field paths that other sets read through an
/// aggregate `{set: it}` or a group over it.
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
        for (_, c) in &set.bind.0 {
            collect(c, &mut out);
        }
        for c in &set.where_ {
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
    out
}

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
    let out_columns: Vec<Clause> = ask.out.columns.clone();
    let out_order: Vec<Clause> = ask.out.order.iter().map(|o| o.0.clone()).collect();
    for name in &validated.order {
        b.current = Some(ask.sets[name].grain);
        b.build_set(name, &out_columns, &out_order)?;
    }
    b.current = ask.sets.get(&ask.out.set).map(|s| s.grain);
    let final_sql = b.answer(&out_columns)?;
    let sql = format!("WITH {} {final_sql}", b.ctes.join(", "));
    Ok(Compiled {
        sql,
        params: b.params,
        columns: b.answer_columns.clone(),
        code_columns: b.code_columns.clone(),
    })
}

impl<'a> Builder<'a> {
    /// The final SELECT over the answer's set.
    fn answer(&mut self, out_columns: &[Clause]) -> R<String> {
        let out = &self.ask.out;
        let set = self
            .ask
            .sets
            .get(&out.set)
            .ok_or_else(|| err("out.set", "no such set"))?;
        let frame = self.frames.get(&out.set).cloned().unwrap_or_default();
        let mut extra: Vec<Clause> = out_columns.to_vec();
        extra.extend(out.order.iter().map(|o| o.0.clone()));
        let terms = self.cte_terms(&out.set, &frame, set, &extra);
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
        for (i, c) in out_columns.iter().enumerate() {
            let e = self.expr(c, &terms, "o", &format!("out.columns[{i}]"))?;
            let name = c
                .ref_name()
                .map(str::to_string)
                .unwrap_or_else(|| format!("col{i}"));
            // a date column reads as text on both backends (H8)
            let rendered = if e.prec.is_some()
                || name == "birth_date"
                || name.ends_with("date")
                || name == "first"
                || name == "last"
            {
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
