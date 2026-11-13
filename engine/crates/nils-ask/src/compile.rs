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
use crate::validate::{Code, ColumnRef, Issue, Names, Validated, WrittenDate};

#[derive(Debug, Clone, PartialEq)]
pub struct CompileError {
    pub path: String,
    pub message: String,
    /// What else the same part of the document is refused for, each at its
    /// own path: the other keys of a group, the other columns and order
    /// keys of the answer. A draft once named three keys of one group in
    /// three refusals, one a turn (2026-11-13).
    pub more: Vec<CompileError>,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)?;
        for e in &self.more {
            write!(f, "\n{}: {}", e.path, e.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for CompileError {}

impl CompileError {
    /// The refusal as an issue of the taxonomy, at the path the compiler
    /// names: what only the compiler refuses of a document, a parameter
    /// without a value for one, is the document's to change, as what the
    /// validator refuses is (2026-10-12).
    pub fn issue(&self) -> Issue {
        Issue {
            code: Code::NotCompilable,
            path: if self.path.is_empty() {
                "document".to_string()
            } else {
                self.path.clone()
            },
            message: self.message.clone(),
            next: "change the document as the message says; validate compiles what run would"
                .to_string(),
        }
    }

    /// Every refusal as an issue, this one first: what one answer tells.
    pub fn issues(&self) -> Vec<Issue> {
        std::iter::once(self)
            .chain(&self.more)
            .map(CompileError::issue)
            .collect()
    }
}

type R<T> = Result<T, CompileError>;

fn err(path: impl Into<String>, message: impl Into<String>) -> CompileError {
    CompileError {
        path: path.into(),
        message: message.into(),
        more: Vec::new(),
    }
}

/// The refusals found in one part of a document as one: the first, with
/// the others beside it in the order they were found. None found, none.
fn together(found: Vec<CompileError>) -> R<()> {
    let mut found = found.into_iter();
    let Some(mut first) = found.next() else {
        return Ok(());
    };
    for mut e in found {
        let more = std::mem::take(&mut e.more);
        first.more.push(e);
        first.more.extend(more);
    }
    Err(first)
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
    /// Record 48, D1 of the move: leave out every stack of a sample sealed
    /// now, at the stack and the instance grains, so no set, count, group
    /// or measure reaches one.
    pub withhold_sealed: bool,
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
    /// Record 55 K7: which answer columns read a quasi identifying field the
    /// caller may not project raw, answered as their shapes.
    pub shaped_columns: Vec<usize>,
    /// Record 55 K7, the ruling of 2026-10-12: which answer columns read a
    /// group's mean or count of distinct values of such a field, answered
    /// for a group of `MEASURE_K` subjects with a value and null where it
    /// is held back.
    pub held_columns: Vec<usize>,
    /// Record 55 K7, small counts at plain (the ruling of 2026-10-12): which
    /// answer columns are counts of rows a quasi identifying field chose or
    /// grouped, held back (null) for fewer than `MEASURE_K` subjects and
    /// beside such a count.
    pub count_columns: Vec<usize>,
    /// Record 55 K7, the ruling of 2026-10-13 (small lists at plain): where
    /// the answer lists the rows of a set a quasi identifying field chose,
    /// below detail quasi, one statement over the same sets that answers 1
    /// where the set holds 1 to 4 subjects, so the list is held back whole;
    /// it binds the first `list_guard_params` of `params`.
    pub list_guard: Option<String>,
    pub list_guard_params: usize,
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

    /// The day an instant falls on, in UTC (2026-10-12): a day and an
    /// instant are compared by their days, the same on both backends.
    pub fn day_of(self, x: &str) -> String {
        if self.is_pg() {
            format!("(({x}) AT TIME ZONE 'UTC')::date")
        } else {
            format!("substr({x}, 1, 10)")
        }
    }

    /// A LIKE that escapes its pattern's own wildcards.
    pub fn like(self, x: &str, pattern_placeholder: &str) -> String {
        format!("{x} LIKE {pattern_placeholder} ESCAPE '\\'")
    }

    /// Wave 7a: whether a name is among a list of names joined by `, ` (the
    /// datasets of a subject or a session). A dataset's name holds no
    /// whitespace, so the separator never falls inside one; no list holds
    /// no name.
    pub fn member(self, list: &str, name: &str) -> String {
        let hay = format!("', ' || COALESCE({list}, '') || ', '");
        let needle = format!("', ' || {name} || ', '");
        if self.is_pg() {
            format!("(strpos({hay}, {needle}) > 0)")
        } else {
            format!("(instr({hay}, {needle}) > 0)")
        }
    }

    /// Wave 7a: names as one list joined by `, `, sorted byte by byte on
    /// both backends so the list reads the same on either; null for none.
    pub fn name_list(self, x: &str) -> String {
        if self.is_pg() {
            format!("string_agg({x}, ', ' ORDER BY {x} COLLATE \"C\")")
        } else {
            format!("group_concat({x}, ', ' ORDER BY {x})")
        }
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

/// A literal that writes no date where a date meets it: a number or a
/// truth, refused there (2026-10-12).
fn not_a_date(a: &Arg) -> Option<String> {
    match a {
        Arg::Int(n) => Some(n.to_string()),
        Arg::Number(n) => Some(n.to_string()),
        Arg::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// A text as an SQL literal, its quotes doubled: for the names of a pack's
/// vocabulary, which are the same on every run.
fn sql_text(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
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
    /// Wave 7a: the value is a list of names joined by `, ` (the datasets
    /// holding a file of a stack, or of a subject's or a session's
    /// stacks), so `=`, `<>`, `in`,
    /// `not_in` and `has` ask whether a name is among them, and a group
    /// keyed by it counts a row once under each name.
    members: bool,
    /// The list is the datasets of the row's own stack, subject or session,
    /// which the layer's spine names, so whether a dataset is among them is
    /// asked of the stacks' holders by the dataset's `source` rows instead
    /// of the list. None where the list was carried from another row (a
    /// partner, an ancestor, a binding, a union).
    holder: Option<Holder>,
    /// The value is a date, a day or an instant (2026-10-12), so a text it
    /// meets is read as a date: Postgres compares no date with text.
    dated: Option<Dated>,
}

/// Whose datasets a list of names is: the row's stack's, or those of the
/// stacks of its subject or its session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Holder {
    Stack,
    Subject,
    Session,
}

/// What a date holds: a day, or an instant written `YYYY-MM-DDTHH:MM:SSZ`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dated {
    Day,
    Instant,
}

impl Term {
    fn plain(sql: String) -> Term {
        Term {
            sql,
            prec: None,
            ci: None,
            param: None,
            members: false,
            holder: None,
            dated: None,
        }
    }

    /// The same term, a day or an instant.
    fn dated(mut self, d: Option<Dated>) -> Term {
        self.dated = d;
        self
    }
}

/// What a column holds when it is a date, from the registry's schema.
fn dated_column(table: &str, column: &str) -> Option<Dated> {
    let ty = nils_registry::schema::registry_tables()
        .iter()
        .find(|t| t.name == table)?
        .column(column)?
        .ty;
    match ty {
        Type::Date => Some(Dated::Day),
        Type::Timestamp => Some(Dated::Instant),
        _ => None,
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
    /// Record 55 K7, the ruling of 2026-10-12: a group's held bindings, each
    /// with the column that counts the subjects whose value it reads.
    held_counts: BTreeMap<String, String>,
    /// Record 55 K7, small counts at plain: per size of the count rule, the
    /// column that is 1 where the group's counts by it are held back.
    count_flags: Vec<String>,
}

fn column_name(binding: &str) -> String {
    format!("b_{}", binding.replace('.', "__"))
}

fn field_col(path: &str) -> String {
    // a measure's path carries its pipeline's name, which may hold a `-`
    // (record 49 A3): a column name does not
    format!("f_{}", path.replace('.', "__").replace('-', "_x_"))
}

/// The base relation of a grain: its FROM clause, its key, and what it
/// carries. The aliases are fixed so a field's column can be named.
struct Base {
    /// The grain's own table, and the joins that hang off it.
    first: String,
    joins: String,
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

/// The joins of a base relation that only some sets read: the labels of a
/// session under the scheme, and the MR detail of a series.
#[derive(Debug, Clone, Copy, Default)]
struct Needs {
    label: bool,
    detail: bool,
    /// The session of a stack or an instance: a field of it, or its key
    /// for a set of sessions to count or pick by.
    session: bool,
    /// The axes of a stack, its row of the read model.
    axes: bool,
}

/// What ties a relation that restricts a set to the set's base: the base's
/// own key, its subject, or its session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    Key,
    Subject,
    Session,
}

/// A relation, or two, that restricts a set: a source, an ancestor, a
/// handle's or a list's members.
struct Lead {
    via: Via,
    /// The relations, in an order where each is tied to the one before.
    rels: Vec<String>,
    /// What ties them to each other and to the base, and what they are
    /// read under.
    on: Vec<String>,
}

/// The name a stack set's frame keeps the values of an axis under: no
/// document writes it, so no field, binding or partner is called it.
fn axis_term(axis: &str) -> String {
    format!("@axis:{axis}")
}

/// Whether an axis's name is one a column and a JSON path can carry as
/// written: letters, digits and underscores, as every axis of a pack is.
fn plain_axis(axis: &str) -> bool {
    !axis.is_empty()
        && !axis.starts_with(|c: char| c.is_ascii_digit())
        && axis.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// What a row's value of an axis is held against: one of some texts, each
/// a placeholder, or a condition on `ax.value`.
enum Held {
    Any(Vec<String>),
    Sql(String),
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

/// A list longer than this is bound as one value, not a placeholder per
/// item.
pub const LIST_BIND_MAX: usize = 1_000;

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
    /// The places in `out.columns` answered as their shapes (record 55 K7),
    /// and their places in the answer.
    shaped: BTreeSet<usize>,
    shaped_columns: Vec<usize>,
    /// The sets whose totals show only for `MEASURE_K` scans or more
    /// (below detail quasi, over a pipeline's measures).
    small_cells: BTreeSet<String>,
    /// Record 55 K7, the ruling of 2026-10-12: the places in `out.columns`
    /// held to a group's subjects, with the bindings whose subjects hold
    /// each, and the held columns' places in the answer.
    held: BTreeMap<usize, BTreeSet<String>>,
    held_columns: Vec<usize>,
    /// Record 55 K7, small counts at plain (the ruling of 2026-10-12): the
    /// sets a quasi identifying field chose or grouped, the rule for the
    /// answer's group counts, and the count columns' places in the answer.
    quasi_sets: BTreeSet<String>,
    counts: crate::validate::CountRule,
    count_columns: Vec<usize>,
    /// Record 55 K7, small lists at plain (the ruling of 2026-10-13): the
    /// select that says the answer's list is held back, and how many of
    /// the parameters the sets bind.
    list_guard: Option<String>,
    list_guard_params: usize,
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
    "n_slices",
    "slice_span_mm",
];

fn cte_name(set: &str) -> String {
    format!("s_{set}")
}

/// Record 55 K7, the ruling of 2026-10-12: how many subjects (rows, where
/// the rows have none) have a value of `x` among the rows `ch`, the number
/// a group's mean or count of distinct values of a quasi identifying field
/// is held to below detail quasi. Subjects, never rows: one subject's five
/// visits are one subject's age.
fn subjects_with(x: &str, has_subj: bool) -> String {
    let who = if has_subj { "ch.subj" } else { "ch.k" };
    format!("COUNT(DISTINCT CASE WHEN ({x}) IS NOT NULL THEN {who} END)")
}

/// The column of a group's layer that counts the subjects of a held binding.
fn held_count_col(binding: &str) -> String {
    format!("kn_{}", binding.replace('.', "__"))
}

/// Record 55 K7, small counts at plain (the ruling of 2026-10-12): the
/// column of the answer's group layer that counts its subjects, whatever
/// else holds its `_subjects`.
const COUNT_SUBJECTS: &str = "kq_subjects";

/// The column of the answer's group layer that counts the subjects a
/// binding counts by a quasi identifying field of its own.
fn own_count_col(binding: &str) -> String {
    format!("kq_{}", binding.replace('.', "__"))
}

/// How many subjects (rows, where the rows have none) of the rows `ch` a
/// count of `x` counts: those with a value, or, for a sum or a mean of a
/// comparison such as the women of a group, those whose value is not 0.
fn subjects_counted(x: &str, nonzero: bool, has_subj: bool) -> String {
    let who = if has_subj { "ch.subj" } else { "ch.k" };
    if nonzero {
        format!("COUNT(DISTINCT CASE WHEN ({x}) <> 0 THEN {who} END)")
    } else {
        format!("COUNT(DISTINCT CASE WHEN ({x}) IS NOT NULL THEN {who} END)")
    }
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

    /// One place of truth for the main scans (Nima, 2026-10-11): the picks
    /// `picked` and a role set read, as a relation in place of the table
    /// (`nils_registry::pick_truth`). A dataset or a cohort named
    /// (`{dataset: NAME}`, `{cohort: NAME}`) reads its own once a run of its
    /// saved rules finished, else the pack's; none named, each subject reads
    /// its own dataset's by the same rule, else the pack's, as the Main
    /// scans page, the viewer and a release do.
    fn main_picks(&self, opts: Option<&BTreeMap<String, Value>>, at: &str) -> R<String> {
        use nils_registry::pick_truth::{Holder, Of, Version};
        let named = |k: &str| opts.and_then(|o| o.get(k));
        let of = match (named("dataset"), named("cohort")) {
            (None, None) => Of::own_of(self.ctx.names.datasets().into_iter().map(|d| Holder {
                id: d.id,
                name: d.name,
                sources: d.sources,
                version: Version::Now,
            })),
            (Some(Value::String(d)), None) => {
                let name = d.trim_start_matches('@');
                let found = self
                    .ctx
                    .names
                    .datasets()
                    .into_iter()
                    .find(|x| x.name == name)
                    .ok_or_else(|| err(at, format!("picked reads no dataset named {name}")))?;
                Of::dataset(found.id, &found.name, found.sources)
            }
            (None, Some(Value::String(c))) => {
                if !self.ctx.names.cohorts().iter().any(|x| x == c) {
                    return Err(err(at, format!("picked reads no cohort named {c}")));
                }
                Of::cohort_named(c)
            }
            (Some(_), Some(_)) => {
                return Err(err(
                    at,
                    "picked reads one scope: a dataset or a cohort, not both",
                ));
            }
            _ => return Err(err(at, "picked names its dataset or its cohort by name")),
        };
        Ok(of.relation(self.ctx.schema.as_deref()))
    }

    /// Record 48, D1 of the move: the standing predicate that leaves out a
    /// stack of a sample sealed now, when the context withholds them.
    fn not_sealed(&self, stack: &str) -> Option<String> {
        self.ctx.withhold_sealed.then(|| {
            format!(
                "NOT EXISTS (SELECT 1 FROM {} sst WHERE sst.stack_id = {stack} AND sst.unsealed_at IS NULL)",
                self.q("sealed_stack")
            )
        })
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

    fn base(&mut self, grain: Grain, path: &str, needs: Needs, anchor: Via) -> R<Base> {
        let q = |b: &Builder, t: &str| b.q(t);
        // the labels of a session and the MR detail of a series are read by
        // a unique key each, so a set that names no field of theirs is the
        // same set without the join
        let label = |b: &mut Builder| -> String {
            if !needs.label {
                return String::new();
            }
            let digest = b.p(Param::from(b.ctx.scheme_digest.as_str()), Type::Text);
            format!(
                " LEFT JOIN {} sl ON sl.session_id = sc.id AND sl.scheme_digest = {digest}",
                b.q("session_label")
            )
        };
        Ok(match grain {
            Grain::Cohort => Base {
                first: format!("{} c", q(self, "cohort")),
                joins: String::new(),
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
                first: format!("{} su", q(self, "subject")),
                joins: String::new(),
                key: "su.id".into(),
                subj: Some("su.id".into()),
                day: None,
                prec: None,
                session_k: None,
                study_k: None,
                series_k: None,
                stack_k: None,
                // record 26 §6: a subject merged into another is not a
                // subject any listing sees; its rows moved with the merge
                standing: vec!["su.merged_into IS NULL".into()],
            },
            Grain::Session => {
                let label = label(self);
                let window = self.p(Param::Int(self.ctx.window_days), Type::Int);
                let (sc, su) = (q(self, "session_cache"), q(self, "subject"));
                let (first, joins) = match anchor {
                    Via::Subject => (
                        format!("{su} su"),
                        format!(" JOIN {sc} sc ON sc.subject_id = su.id{label}"),
                    ),
                    _ => (
                        format!("{sc} sc"),
                        format!(" JOIN {su} su ON su.id = sc.subject_id{label}"),
                    ),
                };
                Base {
                    first,
                    joins,
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
            Grain::Stack | Grain::Instance => {
                let window = if needs.session || anchor == Via::Session {
                    self.p(Param::Int(self.ctx.window_days), Type::Int)
                } else {
                    String::new()
                };
                let label = label(self);
                // the stack's row of the read model, where a clause reads an
                // axis of it
                let axes = if needs.axes && grain == Grain::Stack {
                    format!(
                        " LEFT JOIN {} xa ON xa.stack_id = st.id",
                        q(self, "stack_axis")
                    )
                } else {
                    String::new()
                };
                let detail = if needs.detail && grain == Grain::Stack {
                    format!(
                        " LEFT JOIN {} smr ON smr.series_id = se.id",
                        q(self, "series_mr")
                    )
                } else {
                    String::new()
                };
                // Record 35 finding 1: whose stack this is comes off the
                // **series**, which carries the subject the stack's own
                // files named, and not off the study, which carries
                // whoever the study's files named. The two differ wherever
                // an archive re-linked a series, and reading the study
                // folded one subject's stacks into another's: an ask saw
                // 38 subjects where a selection reached 39. The session
                // follows the same rule, so a stack is never rolled up
                // under a session of somebody else: the session of a
                // stack's study, where it is the session of the stack's
                // own subject. It is joined by its key alone, with the
                // subject held as a condition on the row found and not as
                // a second key: written as two keys, a planner multiplies
                // the two selectivities and takes the join, once a set
                // reads it strictly, for one row in thousands where it is
                // nearly every row, and plans what follows for a handful.
                let own = "NULLIF(sc.subject_id = se.subject_id, FALSE) IS NOT NULL";
                let (series, study, subject) =
                    (q(self, "series"), q(self, "study"), q(self, "subject"));
                let (scs, sc) = (q(self, "session_cache_study"), q(self, "session_cache"));
                // the grain's own rows off a series: a stack with its
                // fingerprint, or an instance (the same rule, for the same
                // reason)
                let (unit, of_series, own_first) = if grain == Grain::Stack {
                    let (st, f) = (q(self, "stack"), q(self, "stack_fingerprint"));
                    (
                        format!("{st} st"),
                        format!(
                            " JOIN {st} st ON st.series_id = se.id JOIN {f} f ON f.stack_id = st.id"
                        ),
                        format!(
                            " JOIN {series} se ON se.id = st.series_id JOIN {f} f ON f.stack_id = st.id"
                        ),
                    )
                } else {
                    let i = q(self, "instance");
                    (
                        format!("{i} i"),
                        format!(" JOIN {i} i ON i.series_id = se.id"),
                        format!(" JOIN {series} se ON se.id = i.series_id"),
                    )
                };
                // a set nothing reads the session of is the same set without
                // it: each study has one session under a window, and each
                // session one row
                let sessions = if needs.session {
                    format!(
                        " LEFT JOIN {scs} scs ON scs.study_id = sy.id AND scs.window_days = {window} \
                         LEFT JOIN {sc} sc ON sc.id = scs.session_id AND {own}"
                    )
                } else {
                    String::new()
                };
                // The joins are written from what restricts the set: each
                // table after the first is tied to one before it, so that
                // a planner which takes a long list of joins in the order
                // written (Postgres past `join_collapse_limit`) joins
                // neighbours and never two tables nothing ties yet. A set
                // of an ancestor's sessions reads the session strictly, so
                // its joins to the session are inner ones.
                let (first, joins) = match anchor {
                    Via::Session => (
                        format!("{sc} sc"),
                        format!(
                            " JOIN {scs} scs ON scs.session_id = sc.id AND scs.window_days = {window} \
                             JOIN {study} sy ON sy.id = scs.study_id \
                             JOIN {series} se ON se.study_id = sy.id AND {own}{of_series} \
                             JOIN {subject} su ON su.id = se.subject_id{axes}{label}{detail}"
                        ),
                    ),
                    Via::Subject => (
                        format!("{subject} su"),
                        format!(
                            " JOIN {series} se ON se.subject_id = su.id{of_series} \
                             JOIN {study} sy ON sy.id = se.study_id{axes}{sessions}{label}{detail}"
                        ),
                    ),
                    Via::Key => (
                        unit,
                        format!(
                            "{own_first} JOIN {subject} su ON su.id = se.subject_id \
                             JOIN {study} sy ON sy.id = se.study_id{axes}{sessions}{label}{detail}"
                        ),
                    ),
                };
                let mut standing = Vec::new();
                let stack = if grain == Grain::Stack {
                    // §4.4 rule 10 and §14.2: a stack the pack ruled out is
                    // not a stack any set sees, and a document cannot switch
                    // it off. Describe prints this predicate, so the query
                    // has to carry it or the two disagree by exactly the
                    // excluded stacks. It reads the few rows of the read
                    // model that say so (`nils_registry::stack_axis`).
                    standing.push(self.not_ruled_out("dax", "st.id"));
                    "st.id"
                } else {
                    "i.stack_id"
                };
                if let Some(sealed) = self.not_sealed(stack) {
                    standing.push(sealed);
                }
                Base {
                    first,
                    joins,
                    key: if grain == Grain::Stack {
                        "st.id"
                    } else {
                        "i.id"
                    }
                    .into(),
                    subj: Some("su.id".into()),
                    day: Some("COALESCE(sy.date_filled, sy.study_date)".into()),
                    prec: None,
                    session_k: (needs.session || anchor == Via::Session)
                        .then(|| "sc.id".to_string()),
                    study_k: Some("sy.id".into()),
                    series_k: Some("se.id".into()),
                    stack_k: Some(stack.into()),
                    standing,
                }
            }
            Grain::Event => {
                let (e, ot, su) = (
                    q(self, "event"),
                    q(self, "observation_type"),
                    q(self, "subject"),
                );
                let (first, joins) = match anchor {
                    Via::Subject => (
                        format!("{su} su"),
                        format!(
                            " JOIN {e} e ON e.subject_id = su.id JOIN {ot} ot ON ot.id = e.observation_type_id"
                        ),
                    ),
                    _ => (
                        format!("{e} e"),
                        format!(
                            " JOIN {ot} ot ON ot.id = e.observation_type_id JOIN {su} su ON su.id = e.subject_id"
                        ),
                    ),
                };
                Base {
                    first,
                    joins,
                    key: "e.id".into(),
                    subj: Some("su.id".into()),
                    day: Some("e.event_date".into()),
                    prec: Some("COALESCE(e.event_date_precision, 'day')".into()),
                    session_k: None,
                    study_k: None,
                    series_k: None,
                    stack_k: None,
                    standing: vec!["e.superseded_by IS NULL".into()],
                }
            }
            Grain::Group | Grain::Pair => {
                return Err(err(path, format!("{grain} has no base relation")));
            }
        })
    }

    /// The column of a field of a level in the base relation.
    fn base_column(&self, level: &str, path: &str) -> Option<Term> {
        let c: ColumnRef = self.ctx.names.column(level, path)?;
        if c.table == "measure" {
            return self.measure_column(level, &c.column);
        }
        if c.table == crate::validate::DATASET_TABLE {
            return self.dataset_column(level);
        }
        if c.table == "study" && c.column == "day" {
            return Some(
                Term::plain("COALESCE(sy.date_filled, sy.study_date)".into())
                    .dated(Some(Dated::Day)),
            );
        }
        if c.table == "session_cache_study" && c.column == "has_primary" {
            return Some(self.session_has_primary());
        }
        let alias = alias_of(&c.table)?;
        let prec = (c.table == "event" && c.column == "event_date")
            .then(|| "COALESCE(e.event_date_precision, 'day')".to_string());
        Some(Term {
            sql: format!("{alias}.{}", c.column),
            prec,
            ci: c.ci.map(|ci| format!("{alias}.{ci}")),
            param: None,
            members: false,
            holder: None,
            dated: dated_column(&c.table, &c.column),
        })
    }

    /// What a session's studies say of it together, which no table keeps:
    /// `has_primary`, as the session resolver folds it (`nils-session`). A
    /// session holds an original primary where a study of it says so; where
    /// none does and one has not said (its stacks are not all
    /// fingerprinted), nobody knows; where every study says no, it holds
    /// none.
    fn session_has_primary(&self) -> Term {
        Term::plain(format!(
            "(SELECT CASE WHEN SUM(CASE WHEN hy.has_original_primary <> 0 THEN 1 ELSE 0 END) > 0 THEN 1 \
             WHEN COUNT(*) > 0 AND COUNT(hy.has_original_primary) = COUNT(*) THEN 0 END \
             FROM {} hs JOIN {} hy ON hy.id = hs.study_id WHERE hs.session_id = sc.id)",
            self.q("session_cache_study"),
            self.q("study"),
        ))
    }

    /// §4.4 rule 10: the stack is not one the pack ruled out, asked of the
    /// rows of the read model that say a stack is (`excluded = 1`, the few
    /// a partial key lists), where every stack's `disposition` rows were
    /// looked up before.
    fn not_ruled_out(&self, alias: &str, stack: &str) -> String {
        format!(
            "NOT EXISTS (SELECT 1 FROM {} {alias} WHERE {alias}.stack_id = {stack} AND {alias}.excluded = 1)",
            self.q("stack_axis")
        )
    }

    /// One axis out of a stack's `axes` document: the array of its values,
    /// null where the stack has no row of the axis.
    fn axis_of(&mut self, doc: &str, axis: &str) -> String {
        if self.sql.is_pg() {
            format!("({doc} -> {})", sql_text(axis))
        } else if plain_axis(axis) {
            format!("json_extract({doc}, '$.{axis}')")
        } else {
            let key = self.p(Param::from(axis), Type::Text);
            format!("(SELECT xj.value FROM json_each({doc}) xj WHERE xj.key = {key})")
        }
    }

    /// The values of an axis of the row `alias` names, as the array the
    /// read model keeps (`nils_registry::stack_axis`): the column the set
    /// projects for the axis, or, where the layer carries none (a set that
    /// did not foresee the reader, a union), the stack's row read by its
    /// key. The values are the rows `classification_axis` holds for the
    /// stack and the axis, one element each, so every reading below
    /// answers what it answered on those rows.
    fn axis_values(
        &mut self,
        axis: &str,
        terms: &[(String, Term)],
        alias: &str,
        at: &str,
    ) -> String {
        if let Ok(t) = self.term(terms, alias, &axis_term(axis), at) {
            return t.sql;
        }
        let k = if alias.is_empty() {
            "k".to_string()
        } else {
            format!("{alias}.k")
        };
        let one = self.axis_of("xa.axes", axis);
        format!(
            "(SELECT {one} FROM {} xa WHERE xa.stack_id = {k})",
            self.q("stack_axis")
        )
    }

    /// An axis's values as rows `alias(value)`, for a FROM clause: one row
    /// a value, a row with no value as null, none where there is no array.
    fn axis_rows(&self, values: &str, alias: &str) -> String {
        if self.sql.is_pg() {
            format!("jsonb_array_elements_text({values}) AS {alias}(value)")
        } else {
            format!("json_each({values}) {alias}")
        }
    }

    /// Whether a row's axis holds a value: one of some texts, or one a
    /// condition on `ax.value` takes. Never null. On SQLite the value is
    /// read as text, as the column of `classification_axis` is, so a
    /// number it is held against is compared as its digits.
    fn axis_holds(&self, values: &str, held: &Held) -> String {
        let cond = match held {
            Held::Any(texts) if self.sql.is_pg() && !texts.is_empty() => {
                return format!("COALESCE({values} ?| ARRAY[{}], FALSE)", texts.join(", "));
            }
            Held::Any(texts) if texts.is_empty() => "ax.value IN (NULL)".to_string(),
            Held::Any(texts) => format!("ax.value IN ({})", texts.join(", ")),
            Held::Sql(cond) => cond.clone(),
        };
        let cond = if self.sql.is_pg() {
            cond
        } else {
            cond.replace("ax.value", "CAST(ax.value AS TEXT)")
        };
        format!(
            "EXISTS (SELECT 1 FROM {} WHERE {cond})",
            self.axis_rows(values, "ax")
        )
    }

    /// A row's values of an axis as one sorted list (H5), each read in the
    /// form `form` gives it: null where it has none. An axis of one value,
    /// as most rows have, is that value in its form, read without a
    /// subquery.
    fn axis_list(&self, values: &str, form: impl Fn(&str) -> String) -> String {
        let all = format!(
            "(SELECT {} FROM {})",
            self.sql.sorted_list(&form("ax.value")),
            self.axis_rows(values, "ax")
        );
        let (count, one) = if self.sql.is_pg() {
            (
                format!("jsonb_array_length({values})"),
                format!("({values} ->> 0)"),
            )
        } else {
            (
                format!("json_array_length({values})"),
                format!("({values} ->> '$[0]')"),
            )
        };
        format!(
            "(CASE WHEN {values} IS NULL THEN NULL WHEN {count} = 1 THEN {} ELSE {all} END)",
            form(&one)
        )
    }

    /// The axes a clause reads of the row it is held against: those it
    /// names, and those of a level whose signature it takes.
    fn axes_in(&self, c: &Clause, out: &mut BTreeSet<String>) {
        let mut all = Vec::new();
        c.walk(&mut all);
        for cl in all {
            match (cl.op.as_str(), cl.ref_name()) {
                ("axis", Some(axis)) => {
                    out.insert(axis.to_string());
                }
                ("derived", Some("signature")) => {
                    let level = cl
                        .opts
                        .get("level")
                        .and_then(Value::as_str)
                        .unwrap_or("loose");
                    if let Some(spec) = self.ctx.names.level_spec(level) {
                        for m in spec.exact.iter().chain(spec.rounded.iter().map(|(k, _)| k)) {
                            if self.ctx.names.axis_values(m).is_some() {
                                out.insert(m.clone());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// The axes a stack set projects: those its own clauses read, the
    /// answer's where it is the answer's set, and those another set reads
    /// of its rows through an aggregate or a group's keys. One a reader
    /// names that is not foreseen here is read by the stack's key instead
    /// ([`Self::axis_values`]), with the same answer.
    fn wanted_axes(&self, name: &str, set: &Set) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for c in clauses_of(set) {
            self.axes_in(c, &mut out);
        }
        let mut elsewhere: Vec<&Clause> = Vec::new();
        for other in self.ask.sets.values() {
            elsewhere.extend(clauses_of(other));
            if let Some(g) = &other.group
                && g.of == name
            {
                for c in &g.by {
                    self.axes_in(c, &mut out);
                }
            }
        }
        elsewhere.extend(self.ask.out.columns.iter());
        elsewhere.extend(self.ask.out.order.iter().map(|o| &o.0));
        if name == self.ask.out.set {
            for c in &self.ask.out.columns {
                self.axes_in(c, &mut out);
            }
            for o in &self.ask.out.order {
                self.axes_in(&o.0, &mut out);
            }
        }
        for c in elsewhere {
            let mut all = Vec::new();
            c.walk(&mut all);
            for cl in all {
                if AGGREGATES.contains(&cl.op.as_str())
                    && cl.opts.get("set").and_then(Value::as_str) == Some(name)
                {
                    for a in &cl.args {
                        if let Arg::Clause(inner) = a {
                            self.axes_in(inner, &mut out);
                        }
                    }
                }
            }
        }
        out.retain(|a| plain_axis(a));
        out
    }

    /// Wave 7a: the `dataset` of a stack, a session or a subject, the names
    /// of the datasets whose tree holds a file of the stack, or of one of
    /// the session's or the subject's stacks, as one sorted list, read
    /// beside the base relation through the stacks' holders. Nima,
    /// 2026-10-12: a dataset is a folder that holds stacks, so a subject is
    /// counted in a dataset when some of its stacks are there, and only a
    /// cohort has members; a stack with a path in two datasets is one stack
    /// in both (record 55), and one with no path in any, or a subject or a
    /// session with no such stack, reads null. The stacks rolled up for a
    /// session or a subject are the ones a stack set sees: none the pack
    /// ruled out, and none of a sample sealed now where those are withheld.
    ///
    /// The term names whose list it is; the list itself is made above the
    /// base relation, from the spine ([`Self::dataset_list`]), and a clause
    /// that asks whether a dataset is among them never makes it
    /// ([`Self::dataset_holds`]).
    fn dataset_column(&self, level: &str) -> Option<Term> {
        let holder = match level {
            "stack" => Holder::Stack,
            "subject" => Holder::Subject,
            "session" => Holder::Session,
            _ => return None,
        };
        let mut term = Term::plain("NULL".into());
        term.members = true;
        term.holder = Some(holder);
        Some(term)
    }

    /// The stacks a holder's datasets are read from: a FROM clause that
    /// ends at their `source_stack` rows (`dsb`), the columns that name the
    /// holder, each with the spine column of a row it is tied to, and the
    /// conditions among the stacks. Whose stack it is comes off its series
    /// (record 35 finding 1), and a session's stacks are its studies' of
    /// its own subject; a stack is the row itself, which its set already
    /// sees. Written as cross joins with their conditions beside, which
    /// SQLite reads in the order written, from the row's own key down to
    /// the holders, where left to choose with no statistics it started
    /// from every stack a source holds.
    fn held_stacks(&self, holder: Holder) -> (String, Vec<(&'static str, &'static str)>, String) {
        let (stack, series, held) = (self.q("stack"), self.q("series"), self.q("source_stack"));
        let standing = || {
            let mut standing = format!(" AND {}", self.not_ruled_out("dsx", "dst.id"));
            if let Some(sealed) = self.not_sealed("dst.id") {
                standing.push_str(&format!(" AND {sealed}"));
            }
            standing
        };
        match holder {
            Holder::Stack => (
                format!("{held} dsb"),
                vec![("dsb.stack_id", "stack_k")],
                "1 = 1".to_string(),
            ),
            Holder::Subject => (
                format!("{series} dse CROSS JOIN {stack} dst CROSS JOIN {held} dsb"),
                vec![("dse.subject_id", "subj")],
                format!(
                    "dst.series_id = dse.id AND dsb.stack_id = dst.id{}",
                    standing()
                ),
            ),
            Holder::Session => (
                format!(
                    "{} dscs CROSS JOIN {series} dse CROSS JOIN {stack} dst CROSS JOIN {held} dsb",
                    self.q("session_cache_study")
                ),
                vec![("dscs.session_id", "session_k"), ("dse.subject_id", "subj")],
                format!(
                    "dse.study_id = dscs.study_id AND dst.series_id = dse.id \
                     AND dsb.stack_id = dst.id{}",
                    standing()
                ),
            ),
        }
    }

    /// [`Self::held_stacks`] for the one row `alias` names: the FROM clause
    /// and the conditions that tie it to the row.
    fn held_stacks_of(&self, holder: Holder, alias: &str) -> (String, String) {
        let (from, ties, rest) = self.held_stacks(holder);
        let tied: Vec<String> = ties
            .iter()
            .map(|(mine, spine)| format!("{mine} = {alias}.{spine}"))
            .collect();
        (from, format!("{} AND {rest}", tied.join(" AND ")))
    }

    /// Whether a group keyed by a row's own datasets joins its rows to
    /// them, and so never reads the list: on Postgres always; on SQLite,
    /// which joins no relation made per row and reads one made for all
    /// rows once per row, only for a stack's, where every dataset has one
    /// `source` row at most and the stack's holders are its datasets.
    fn joins_datasets(&self, holder: Holder) -> bool {
        self.sql.is_pg() || (holder == Holder::Stack && self.one_source_each())
    }

    /// Every dataset has one `source` row at most, so a stack's holders
    /// among them are its datasets, each once.
    fn one_source_each(&self) -> bool {
        self.ctx
            .names
            .datasets()
            .iter()
            .all(|d| d.sources.len() <= 1)
    }

    /// Each source of a dataset as the dataset's name, over `dsb.source_id`,
    /// and the sources of every dataset; none where no dataset holds one.
    fn dataset_names(&self) -> Option<(String, String)> {
        let quote = |t: &str| format!("'{}'", t.replace('\'', "''"));
        let mut whens = String::new();
        let mut sources: Vec<String> = Vec::new();
        for d in self.ctx.names.datasets() {
            for s in &d.sources {
                whens.push_str(&format!(" WHEN {s} THEN {}", quote(&d.name)));
                sources.push(s.to_string());
            }
        }
        (!sources.is_empty())
            .then(|| (format!("CASE dsb.source_id{whens} END"), sources.join(", ")))
    }

    /// The datasets of the row `alias` names, as one sorted list of names:
    /// null where no dataset holds a file of its stacks.
    fn dataset_list(&self, holder: Holder, alias: &str) -> String {
        let Some((name, sources)) = self.dataset_names() else {
            // no dataset holds a source, so no stack is in one
            return "CAST(NULL AS TEXT)".into();
        };
        let (from, unit) = self.held_stacks_of(holder, alias);
        format!(
            "(SELECT {} FROM (SELECT DISTINCT {name} AS n FROM {from} \
             WHERE {unit} AND dsb.source_id IN ({sources})) dsn)",
            self.sql.name_list("dsn.n"),
        )
    }

    /// Every holder with each dataset that holds a file of its stacks, one
    /// row a pair: the relation a group keyed by `dataset` joins its rows
    /// to, so that a row counts once under each of its datasets (and, by a
    /// left join, once under none). The holder's columns are `hk0`, `hk1`;
    /// the conditions tie them to the spine of the rows `alias` names. Made
    /// once for all rows, where looking each name up in each row's list
    /// made the list once per row and name.
    fn dataset_pairs(&self, holder: Holder, alias: &str, pairs: &str) -> Option<(String, String)> {
        let (name, sources) = self.dataset_names()?;
        let (from, ties, rest) = self.held_stacks(holder);
        let keys: Vec<String> = ties
            .iter()
            .enumerate()
            .map(|(i, (mine, _))| format!("{mine} AS hk{i}"))
            .collect();
        let on: Vec<String> = ties
            .iter()
            .enumerate()
            .map(|(i, (_, spine))| format!("{pairs}.hk{i} = {alias}.{spine}"))
            .collect();
        Some((
            format!(
                "(SELECT DISTINCT {}, {name} AS n FROM {from} WHERE {rest} \
                 AND dsb.source_id IN ({sources}))",
                keys.join(", ")
            ),
            on.join(" AND "),
        ))
    }

    /// Whether one of the datasets named is among those of the row `alias`
    /// names: whether one of their `source` rows holds a file of a stack of
    /// it. The same answer as looking the name up in the list of
    /// [`Self::dataset_list`], asked of the holders by their key instead of
    /// made, sorted and searched as text for every row.
    fn dataset_holds(&self, holder: Holder, alias: &str, names: &[String]) -> String {
        let mut sources: Vec<String> = Vec::new();
        for d in self.ctx.names.datasets() {
            if names.contains(&d.name) {
                sources.extend(d.sources.iter().map(i64::to_string));
            }
        }
        if sources.is_empty() {
            return "(1 = 0)".into();
        }
        let (from, unit) = self.held_stacks_of(holder, alias);
        format!(
            "EXISTS (SELECT 1 FROM {from} WHERE {unit} AND dsb.source_id IN ({}))",
            sources.join(", ")
        )
    }

    /// Whether one of the datasets a clause names outright is among those
    /// of the row, where the field it compares is the row's own list: asked
    /// of the stacks' holders. None for any other clause, which reads the
    /// list.
    fn dataset_among(
        &self,
        c: &Clause,
        terms: &[(String, Term)],
        alias: &str,
        at: &str,
    ) -> Option<String> {
        let (path, named) = self.dataset_named(c)?;
        let holder = self.term(terms, alias, path, at).ok()?.holder?;
        Some(self.dataset_holds(holder, alias, &named))
    }

    /// The dataset field a clause compares with datasets it names outright,
    /// and those names: `=`, `<>`, `in`, `not_in` or `has` of a field and a
    /// text or a list of texts, each the name of a dataset of the registry;
    /// or `is_null` or `not_null` of the field, which name every dataset.
    /// Such a clause asks whether a dataset is among a row's, and never
    /// reads the list; any other reading of the field does.
    fn dataset_named<'c>(&self, c: &'c Clause) -> Option<(&'c str, Vec<String>)> {
        let field = |a: &'c Arg| match a {
            Arg::Clause(l) if l.op == "field" => l.ref_name(),
            _ => None,
        };
        let (path, named) = match (c.op.as_str(), c.args.first(), c.args.get(1)) {
            // whether there is one at all: whether any dataset is among them
            ("is_null" | "not_null", Some(l), None) => (
                field(l)?,
                self.ctx
                    .names
                    .datasets()
                    .into_iter()
                    .map(|d| d.name)
                    .collect(),
            ),
            ("=" | "<>" | "has", Some(l), Some(Arg::Text(t))) => (field(l)?, vec![t.clone()]),
            ("=" | "<>", Some(Arg::Text(t)), Some(r)) => (field(r)?, vec![t.clone()]),
            ("in" | "not_in", Some(l), Some(Arg::List(items))) => {
                let mut named = Vec::with_capacity(items.len());
                for i in items {
                    named.push(i.as_text()?.to_string());
                }
                (field(l)?, named)
            }
            _ => return None,
        };
        if c.args.len() > 2 || named.is_empty() {
            return None;
        }
        let known = self.ctx.names.datasets();
        named
            .iter()
            .all(|n| known.iter().any(|d| d.name == *n))
            .then_some((path, named))
    }

    /// Whether a clause reads the list of datasets at `path` as a list:
    /// anywhere but in a comparison with datasets named outright.
    fn reads_list(&self, c: &Clause, path: &str) -> bool {
        if c.op == "field" {
            return c.ref_name() == Some(path);
        }
        if self.dataset_named(c).is_some_and(|(p, _)| p == path) {
            return false;
        }
        let in_opts = c
            .opts
            .values()
            .any(|v| crate::ast::clause_of(v).is_ok_and(|inner| self.reads_list(&inner, path)));
        in_opts
            || c.args.iter().any(|a| match a {
                Arg::Clause(inner) => self.reads_list(inner, path),
                _ => false,
            })
    }

    /// Whether anything reads the list of datasets at `path` of a set once
    /// its `where` is cut: a clause of its own that reads it as a list, the
    /// answer, another set through an aggregate, a group or a partner, or a
    /// union the set is an operand of.
    fn list_is_read(&self, name: &str, set: &Set, path: &str, holder: Holder) -> bool {
        // a partner carries every column of the set it is of
        let all = || self.ask.sets.values();
        if all()
            .any(|s| s.near.iter().any(|n| n.set == name) || s.attach.iter().any(|a| a.set == name))
        {
            return true;
        }
        if self.external.get(name).is_some_and(|e| e.contains(path)) {
            // another set reads it: an aggregate reads it as its clause
            // does; a group keyed by it joins the rows to their datasets
            // and never reads the list
            let mut clauses: Vec<&Clause> = all().flat_map(clauses_of).collect();
            clauses.extend(self.ask.out.columns.iter());
            clauses.extend(self.ask.out.order.iter().map(|o| &o.0));
            for c in clauses {
                let mut inside = Vec::new();
                c.walk(&mut inside);
                for cl in inside {
                    if AGGREGATES.contains(&cl.op.as_str())
                        && cl.opts.get("set").and_then(Value::as_str) == Some(name)
                        && cl.args.iter().any(|a| match a {
                            Arg::Clause(inner) => self.reads_list(inner, path),
                            _ => false,
                        })
                    {
                        return true;
                    }
                }
            }
            for g in all().filter_map(|s| s.group.as_ref()) {
                if g.of == name
                    && g.by.iter().any(|c| {
                        let key = c.op == "field" && c.ref_name() == Some(path);
                        !(key && self.joins_datasets(holder)) && self.reads_list(c, path)
                    })
                {
                    return true;
                }
            }
        }
        if self.ask.sets.values().any(|s| {
            s.algebra
                .as_ref()
                .is_some_and(|a| a.sets.iter().any(|o| o == name))
        }) {
            return true;
        }
        if clauses_of(set).iter().any(|c| self.reads_list(c, path)) {
            return true;
        }
        if name != self.ask.out.set {
            return false;
        }
        let mut named = BTreeSet::new();
        for c in &self.ask.out.columns {
            paths_in(c, &mut named);
        }
        for o in &self.ask.out.order {
            paths_in(&o.0, &mut named);
        }
        named.contains(path)
    }

    /// A run's measure of a unit (record 49 A3), `<number|text|run>:
    /// <pipeline>:<name>` as the catalog names it: the value of the newest
    /// run that measured the unit, read beside the base relation by the
    /// unit's key (a stack, a subject and a session's day, or a subject).
    /// Record 55 H3: `checked:<pipeline>:`, the status a check left on that
    /// run's results of the unit, the worst of them (rejected, flagged,
    /// unchecked, accepted); none where the run made no mask for it.
    fn measure_column(&self, level: &str, column: &str) -> Option<Term> {
        let mut parts = column.splitn(3, ':');
        let (what, pipeline, name) = (parts.next()?, parts.next()?, parts.next()?);
        let quote = |t: &str| format!("'{}'", t.replace('\'', "''"));
        let value = match what {
            "number" => "nm.number",
            "text" => "nm.text",
            "run" => "nm.run_id",
            "checked" | "key" => "",
            _ => return None,
        };
        let unit = |a: &str| match level {
            "stack" => Some(format!("{a}.stack_id = st.id")),
            "session" => Some(format!(
                "{a}.subject_id = su.id AND {a}.session_day = sc.first"
            )),
            "subject" => Some(format!("{a}.subject_id = su.id")),
            _ => None,
        };
        let (mu, ru, du) = (unit("nm")?, unit("mr")?, unit("dd")?);
        // record 48, D1 of the move: a unit's measure is what a system said
        // of the stacks it was given, so where a sealed sample's stacks are
        // withheld, a session's or a subject's that holds one reads null,
        // its run and its check too (a stack's own leaves with the stack,
        // by the base's standing predicate)
        let guard = match (self.ctx.withhold_sealed, level) {
            (true, "session") => format!(
                " AND NOT EXISTS (SELECT 1 FROM {} mcs JOIN {} mse ON mse.study_id = mcs.study_id \
                 JOIN {} mst ON mst.series_id = mse.id JOIN {} mss ON mss.stack_id = mst.id \
                 WHERE mcs.session_id = sc.id AND mse.subject_id = sc.subject_id \
                 AND mss.unsealed_at IS NULL)",
                self.q("session_cache_study"),
                self.q("series"),
                self.q("stack"),
                self.q("sealed_stack")
            ),
            (true, "subject") => format!(
                " AND NOT EXISTS (SELECT 1 FROM {} mss WHERE mss.subject_id = su.id \
                 AND mss.unsealed_at IS NULL)",
                self.q("sealed_stack")
            ),
            _ => String::new(),
        };
        // the run a unit's values are read from: its newest run of the
        // pipeline's current version (the newest active one), else its
        // newest run of any version; every value, and the run field, come
        // from that one run
        let current = format!(
            "(SELECT pl.id FROM {} pl WHERE pl.name = {} AND pl.state = 'active' ORDER BY pl.version DESC LIMIT 1)",
            self.q("pipeline"),
            quote(pipeline)
        );
        // the guard rides on the run, which every value and the check read
        // their row by
        let run = format!(
            "(SELECT mr.run_id FROM {} mr WHERE mr.pipeline = {} AND mr.scope = {} AND {ru}{guard} \
             ORDER BY CASE WHEN mr.pipeline_id = {current} THEN 1 ELSE 0 END DESC, mr.run_id DESC LIMIT 1)",
            self.q("measure"),
            quote(pipeline),
            quote(level),
        );
        if what == "run" {
            return Some(Term::plain(run));
        }
        // record 57 P3: the comparability key of that run's results of the
        // unit, from the unit's row
        if what == "key" {
            return Some(Term::plain(format!(
                "(SELECT pu.comparability_key FROM {} pu WHERE pu.run_id = {run} AND pu.unit = \
                 (SELECT nm.unit_id FROM {} nm WHERE nm.pipeline = {} AND nm.scope = {} \
                 AND {mu} AND nm.run_id = {run} ORDER BY nm.id DESC LIMIT 1))",
                self.q("pipeline_unit"),
                self.q("measure"),
                quote(pipeline),
                quote(level),
            )));
        }
        if what == "checked" {
            return Some(Term::plain(format!(
                "(SELECT CASE MIN(CASE dd.check_status WHEN 'rejected' THEN 0 WHEN 'flagged' THEN 1 \
                 WHEN 'accepted' THEN 3 ELSE 2 END) WHEN 0 THEN 'rejected' WHEN 1 THEN 'flagged' \
                 WHEN 2 THEN 'unchecked' WHEN 3 THEN 'accepted' END FROM {} dd \
                 WHERE dd.run_id = {run} AND dd.kind = 'mask' AND dd.withdrawn_at IS NULL AND {du})",
                self.q("derivative"),
            )));
        }
        Some(Term::plain(format!(
            "(SELECT {value} FROM {} nm WHERE nm.pipeline = {} AND nm.name = {} AND nm.scope = {} \
             AND {mu} AND nm.run_id = {run} ORDER BY nm.id DESC LIMIT 1)",
            self.q("measure"),
            quote(pipeline),
            quote(name),
            quote(level),
        )))
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
                return (rest == "id" && frame.of.is_some())
                    .then(|| Term::plain("cm.cohort_id".into()));
            }
            return self.base_column(first, rest);
        }
        if let Some(of) = &frame.of
            && of == first
            && let Some(rest) = rest
        {
            let ancestor = self.ask.sets.get(of)?;
            if ancestor.grain == Grain::Cohort {
                return (rest == "id").then(|| Term::plain("cm.cohort_id".into()));
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
        // the ancestor a field path may be read through: the set's own, or
        // the one its source carries
        let inherited = match &set.from {
            Some(Src::Set(s)) => self.frames.get(s).and_then(|f| f.of.clone()),
            _ => None,
        };
        let mut frame = Frame {
            of: set.of.clone().or(inherited),
            ..Frame::default()
        };
        // the fields this set and its readers name, and which of the base's
        // optional joins they read
        let wanted = self.wanted_paths(name, set);
        let mut needs = Needs::default();
        for p in &wanted {
            if let Some(t) = self.field_term(set, &frame, p) {
                for x in [Some(&t.sql), t.prec.as_ref(), t.ci.as_ref()]
                    .into_iter()
                    .flatten()
                {
                    needs.label |= x.starts_with("sl.");
                    needs.detail |= x.starts_with("smr.");
                    needs.session |= x.contains("sc.") || x.starts_with("sl.");
                }
                needs.session |= t.holder == Some(Holder::Session);
            }
        }
        // the session's key is read where a set of sessions counts, picks
        // or attaches by it, or a union passes it on
        needs.session |= set.pick.as_ref().is_some_and(|p| p.per == Grain::Session)
            || self.ask.sets.values().any(|s| {
                s.grain == Grain::Session || s.grain == Grain::Pair || s.algebra.is_some()
            });
        // What restricts the set: a source, an ancestor, a handle's or a
        // list's members, each tied to the base by its key, its subject or
        // its session.
        let mut leads: Vec<Lead> = Vec::new();
        let key = match set.grain {
            Grain::Cohort => "c.id",
            Grain::Subject => "su.id",
            Grain::Session => "sc.id",
            Grain::Stack => "st.id",
            Grain::Instance => "i.id",
            Grain::Event => "e.id",
            Grain::Group | Grain::Pair => "",
        };
        // a source: its bindings and partners come along
        let mut carried_from: Vec<String> = Vec::new();
        let mut terms: Vec<(String, Term)> = Vec::new();
        match &set.from {
            None => {}
            Some(Src::Set(s)) => {
                leads.push(Lead {
                    via: Via::Key,
                    rels: vec![format!("{} x", cte_name(s))],
                    on: vec![format!("x.k = {key}")],
                });
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
                                carried_from.push(format!("x.{c} AS {c}"));
                            }
                        }
                        let mut t = t.clone();
                        t.holder = None;
                        terms.push((n.clone(), t));
                    }
                }
                frame.bindings = src.bindings.clone();
                frame.picked = src.picked;
            }
            Some(Src::Role(r)) => {
                if set.grain != Grain::Stack {
                    return Err(err(format!("{path}.from"), "a role is a stack set"));
                }
                let role = self.p(Param::from(r.as_str()), Type::Text);
                // the main scans, as `picked` reads them with no scope named
                let picks = self.main_picks(None, &format!("{path}.from"))?;
                leads.push(Lead {
                    via: Via::Key,
                    rels: vec![format!("{picks} p"), format!("{} ps", self.q("pick_stack"))],
                    on: vec![
                        format!("p.role = {role}"),
                        "ps.pick_id = p.id".into(),
                        "ps.stack_id = st.id".into(),
                    ],
                });
            }
            Some(Src::Handle { id, .. }) => {
                let hid: i64 = id.parse().map_err(|_| {
                    err(
                        format!("{path}.from"),
                        format!("handle {id} is not a number"),
                    )
                })?;
                let h = self.p(Param::Int(hid), Type::Int);
                leads.push(Lead {
                    via: Via::Key,
                    rels: vec![format!("{} hm", self.q("handle_member"))],
                    on: vec![format!("hm.handle_id = {h}"), format!("hm.key = {key}")],
                });
            }
            Some(Src::Values(v)) => {
                let decl = self.ask.values.get(v).ok_or_else(|| {
                    err(
                        format!("{path}.from"),
                        format!("values:{v} is not declared"),
                    )
                })?;
                let upload = self.p(Param::from(decl.upload.as_str()), Type::Text);
                leads.push(Lead {
                    via: Via::Subject,
                    rels: vec![
                        format!("{} vs", self.q("values_source")),
                        format!("{} vm", self.q("values_member")),
                    ],
                    on: vec![
                        format!("vs.upload_id = {upload}"),
                        "vm.source_id = vs.id".into(),
                        "vm.subject_id = su.id".into(),
                    ],
                });
            }
            Some(Src::Selection { .. }) => {
                return Err(err(
                    format!("{path}.from"),
                    "a selection source lands with slice 7, the stored ask",
                ));
            }
        }
        // the ancestor
        let mut carried_of: Vec<String> = Vec::new();
        if let Some(of) = &set.of {
            let ancestor = self
                .ask
                .sets
                .get(of)
                .ok_or_else(|| err(format!("{path}.of"), format!("no set {of}")))?;
            let anc = self.frames.get(of).cloned().unwrap_or_default();
            let carries = anc
                .terms
                .iter()
                .any(|(_, t)| t.sql.starts_with("b_") || t.sql.starts_with("n_"));
            let a = format!("{} a", cte_name(of));
            match (ancestor.grain, set.grain) {
                // The cohorts of a set are a few rows, and which of them
                // they are is not known when the statement is planned:
                // joined, the members read are taken for an even share of
                // all memberships, a handful where a site's one large
                // cohort holds thousands, and every join after it is
                // planned for that handful. Asked as a lookup the planner
                // does not see through, the share is taken for half, and a
                // set of a few rows is the cheaper mistake. The lookup
                // reads the cohorts once, however many rows ask.
                (Grain::Cohort, _) if !carries => leads.push(Lead {
                    via: Via::Subject,
                    rels: vec![format!("{} cm", self.q("cohort_member"))],
                    on: vec![
                        format!(
                            "(cm.cohort_id IN (SELECT a.k FROM {} a)) IS TRUE",
                            cte_name(of)
                        ),
                        "cm.left_at IS NULL".into(),
                        "cm.subject_id = su.id".into(),
                    ],
                }),
                (Grain::Cohort, _) => leads.push(Lead {
                    via: Via::Subject,
                    rels: vec![a, format!("{} cm", self.q("cohort_member"))],
                    on: vec![
                        "cm.cohort_id = a.k".into(),
                        "cm.left_at IS NULL".into(),
                        "cm.subject_id = su.id".into(),
                    ],
                }),
                (Grain::Subject, _) => leads.push(Lead {
                    via: Via::Subject,
                    rels: vec![a],
                    on: vec!["a.k = su.id".into()],
                }),
                (Grain::Session, Grain::Stack | Grain::Instance) => leads.push(Lead {
                    via: Via::Session,
                    rels: vec![a],
                    on: vec!["a.k = sc.id".into()],
                }),
                (Grain::Stack, Grain::Instance) => leads.push(Lead {
                    via: Via::Key,
                    rels: vec![a],
                    on: vec!["a.k = i.stack_id".into()],
                }),
                (g, h) => {
                    return Err(err(
                        format!("{path}.of"),
                        format!("{g} is not an ancestor of {h}"),
                    ));
                }
            }
            let mut carried: BTreeSet<String> = BTreeSet::new();
            for (n, t) in &anc.terms {
                if t.sql.starts_with("b_") || t.sql.starts_with("n_") {
                    let mine = format!("o_{}", t.sql);
                    if carried.insert(mine.clone()) {
                        carried_of.push(format!("a.{} AS {mine}", t.sql));
                    }
                    let prec = t.prec.as_ref().map(|p| {
                        let pc = format!("o_{p}");
                        if carried.insert(pc.clone()) {
                            carried_of.push(format!("a.{p} AS {pc}"));
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
                            members: t.members,
                            holder: None,
                            dated: t.dated,
                        },
                    ));
                }
            }
            if ancestor.grain == Grain::Cohort {
                carried_of.push("cm.cohort_id AS cohort_k".into());
                terms.push(("cohort.id".into(), Term::plain("cohort_k".into())));
            }
        }
        // The one that comes first in the FROM clause, with the base
        // written from the table it ties to: a source or a handle by the
        // key, else an ancestor's sessions, else its subjects. A planner
        // that takes a long list of joins in the order written then starts
        // from the rows that restrict the set, where the base written
        // first made it join every row of the grain before it met them.
        let first = [Via::Key, Via::Session, Via::Subject]
            .iter()
            .find_map(|v| leads.iter().position(|l| l.via == *v));
        let anchor = first.map_or(Via::Key, |i| leads[i].via);
        // the axes of a stack this set or a reader of it reads, each a
        // column beside the fields
        let axes = if set.grain == Grain::Stack {
            self.wanted_axes(name, set)
        } else {
            BTreeSet::new()
        };
        needs.axes = !axes.is_empty();
        let base = self.base(set.grain, &path, needs, anchor)?;
        let mut from = String::new();
        let mut wheres: Vec<String> = Vec::new();
        let join = |from: &mut String, rel: &str| {
            if from.is_empty() {
                from.push_str(rel);
            } else {
                from.push_str(&format!(" JOIN {rel} ON 1 = 1"));
            }
        };
        if let Some(i) = first {
            let lead = leads.remove(i);
            for rel in &lead.rels {
                join(&mut from, rel);
            }
            wheres.extend(lead.on);
        }
        join(&mut from, &base.first);
        from.push_str(&base.joins);
        for lead in leads {
            for rel in &lead.rels {
                join(&mut from, rel);
            }
            wheres.extend(lead.on);
        }
        frame.has_subj = base.subj.is_some();
        frame.has_day = base.day.is_some();
        frame.has_prec = base.prec.is_some();
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
        projected.extend(carried_from);
        projected.extend(carried_of);
        for axis in &axes {
            let col = format!("x_{axis}");
            projected.push(format!("{} AS {col}", self.axis_of("xa.axes", axis)));
            terms.push((axis_term(axis), Term::plain(col)));
        }
        // the fields this set and its readers name, projected once. The
        // datasets of the row are a list made by a subquery per row: they
        // are read above this layer by the spine, before the clauses where
        // one reads the list itself and after `where` otherwise, and not at
        // all where every clause asks only whether a dataset is among them
        let mut lists: Vec<(String, Holder, String)> = Vec::new();
        for p in &wanted {
            if terms.iter().any(|(n, _)| n == p) {
                continue;
            }
            if let Some(t) = self.field_term(set, &frame, p) {
                let col = field_col(p);
                let mut layered = Term::plain(col.clone());
                layered.members = t.members;
                layered.holder = t.holder;
                layered.dated = t.dated;
                if let Some(holder) = t.holder {
                    lists.push((p.clone(), holder, col));
                    terms.push((p.clone(), layered));
                    continue;
                }
                projected.push(format!("{} AS {col}", t.sql));
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
                    members: false,
                    holder: None,
                    dated: Some(Dated::Day),
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
        wheres.extend(base.standing.iter().cloned());
        let mut layer = format!("SELECT {} FROM {from}", projected.join(", "));
        if !wheres.is_empty() {
            layer.push_str(&format!(" WHERE {}", wheres.join(" AND ")));
        }

        // The bindings `where` may be read before: none of those it names,
        // by themselves or through another, and none at or before the last
        // one that reads the rows around its own (`ordinal`, `prev`, `next`)
        // or a subject's history (`change`), which count the rows `where`
        // has not cut yet.
        let binds: Vec<(&String, &Clause)> = set.bind.0.iter().map(|(b, c)| (b, c)).collect();
        let reads = |c: &Clause, b: &str| -> bool {
            let mut paths = BTreeSet::new();
            paths_in(c, &mut paths);
            paths
                .iter()
                .any(|p| p == b || p.strip_prefix(b).is_some_and(|r| r.starts_with('.')))
        };
        let mut early: Vec<bool> = binds
            .iter()
            .map(|(b, _)| set.where_.iter().any(|c| reads(c, b)))
            .collect();
        if let Some(last) = binds.iter().rposition(|(_, c)| {
            let mut all = Vec::new();
            c.walk(&mut all);
            all.iter()
                .any(|cl| matches!(cl.op.as_str(), "ordinal" | "prev" | "next" | "change"))
        }) {
            for e in early.iter_mut().take(last + 1) {
                *e = true;
            }
        }
        loop {
            let mut changed = false;
            for i in 0..binds.len() {
                if early[i] {
                    for j in 0..i {
                        if !early[j] && reads(binds[i].1, binds[j].0) {
                            early[j] = true;
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        // a list read as a list before `where` is cut is made first
        let mut read_early: Vec<&Clause> = Vec::new();
        for n in &set.near {
            read_early.extend(n.order.iter().map(|o| &o.0));
        }
        for (i, (_, c)) in binds.iter().enumerate() {
            if early[i] {
                read_early.push(c);
            }
        }
        read_early.extend(set.where_.iter());
        let mut after_where: Vec<(String, Holder, String)> = Vec::new();
        for (p, holder, col) in lists {
            let as_list = read_early.iter().any(|c| self.reads_list(c, &p))
                || set.near.iter().any(|n| n.on.as_deref() == Some(p.as_str()))
                || set.has.iter().any(|h| h.on.as_deref() == Some(p.as_str()));
            if as_list {
                layer = format!(
                    "SELECT q.*, {} AS {col} FROM ({layer}) q",
                    self.dataset_list(holder, "q")
                );
            } else {
                after_where.push((p, holder, col));
            }
        }

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
            let whole = h.window.is_none() && h.on.is_none();
            // whether there is one, or none: asked as that, which a planner
            // answers from either side, where a count per row is a pass
            // over the child's rows for every row
            let asked = (
                h.as_.is_some(),
                h.min.as_ref().map(|m| matches!(m, IntSpec::Literal(1))),
                h.max.as_ref().map(|m| matches!(m, IntSpec::Literal(0))),
            );
            if whole && matches!(asked, (false, Some(true), None) | (false, None, Some(true))) {
                let link = self.count_link(set, &h.set, "ch", &hp)?;
                has_preds.push(format!(
                    "{}EXISTS (SELECT 1 FROM {} ch WHERE {link} = q.k)",
                    if h.min.is_some() { "" } else { "NOT " },
                    cte_name(&h.set)
                ));
                continue;
            }
            // a count of all of a child's rows per row: the child counted
            // once by the key that ties it, and joined
            let count = if whole {
                let link = self.count_link(set, &h.set, "ch", &hp)?;
                let col = format!("h_{i}");
                layer = format!(
                    "SELECT q.*, COALESCE(h.n, 0) AS {col} FROM ({layer}) q LEFT JOIN \
                     (SELECT {link} AS hk, COUNT(*) AS n FROM {} ch GROUP BY {link}) h ON h.hk = q.k",
                    cte_name(&h.set)
                );
                format!("q.{col}")
            } else {
                self.count_of(set, &h.set, h.window.as_ref(), h.on.as_deref(), &terms, &hp)?
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
        // bind, one layer each so a later one reads an earlier one: first
        // those `where` reads or may not come before, then `where`, then
        // the rest, on the rows that are left
        for late in [false, true] {
            for (i, (b, c)) in binds.iter().enumerate() {
                if early[i] == late {
                    continue;
                }
                let bp = format!("{path}.bind.{b}");
                if c.op == "change" {
                    layer = self.change_layer(set, b, c, &bp, layer, &mut terms, &mut frame)?;
                    continue;
                }
                let expr = self.expr(c, &terms, "q", &bp)?;
                let col = column_name(b);
                layer = format!("SELECT q.*, {} AS {col} FROM ({layer}) q", expr.sql);
                frame.bindings.push(((*b).clone(), col.clone()));
                terms.push((
                    (*b).clone(),
                    Term {
                        sql: col,
                        prec: expr.prec,
                        ci: None,
                        param: None,
                        members: expr.members,
                        holder: None,
                        dated: expr.dated,
                    },
                ));
            }
            if late {
                break;
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
            // the lists nothing read before, where a later binding, the
            // pick, the answer or another set reads one
            for (p, holder, col) in std::mem::take(&mut after_where) {
                if self.list_is_read(name, set, &p, holder) {
                    layer = format!(
                        "SELECT q.*, {} AS {col} FROM ({layer}) q",
                        self.dataset_list(holder, "q")
                    );
                }
            }
        }
        // a binding keeps the place it was written at, for a union's columns
        let order: Vec<&String> = binds.iter().map(|(b, _)| *b).collect();
        let place = |b: &str| order.iter().position(|o| o.as_str() == b);
        if !order.is_empty() {
            let first = frame
                .bindings
                .iter()
                .position(|(b, _)| place(b).is_some())
                .unwrap_or(frame.bindings.len());
            frame.bindings[first..].sort_by_key(|(b, _)| place(b).unwrap_or(usize::MAX));
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
                    members: t.members,
                    holder: None,
                    dated: t.dated,
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
                    members: false,
                    holder: None,
                    dated: (n == "date").then_some(Dated::Day),
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
                                members: t.members,
                                holder: None,
                                dated: t.dated,
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
                            members: t.members,
                            holder: None,
                            dated: t.dated,
                        },
                    ));
                }
                both.push((
                    format!("{}.date", n.as_),
                    Term::plain(p_first.clone()).dated(Some(Dated::Day)),
                ));
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

    /// The column of a child set that holds the key of the row of this set
    /// each of its rows is counted under: a group's key by this grain, the
    /// key itself for a set of the same grain, else the spine's link.
    fn count_link(&self, set: &Set, child_name: &str, alias: &str, at: &str) -> R<String> {
        let child = self.frames.get(child_name).cloned().unwrap_or_default();
        let child_set = &self.ask.sets[child_name];
        if child_set.grain == Grain::Group {
            let key_path = format!("{}.id", set.grain.name());
            let (_, gcol) = child
                .group_by
                .iter()
                .find(|(p, _)| p == &key_path)
                .ok_or_else(|| err(at, "the group is not keyed by this grain"))?;
            return Ok(format!("{alias}.{gcol}"));
        }
        if child_set.grain == set.grain {
            return Ok(format!("{alias}.k"));
        }
        Self::link(&child, set.grain, alias, at)
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
        let link = self.count_link(set, child_name, "ch", at)?;
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
            members: false,
            holder: None,
            dated: Some(Dated::Day),
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
            Term::plain(format!("{col}__from_date")).dated(Some(Dated::Day)),
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
        // Record 38 S4: an axis is a key a group may be read by. The value
        // is a stack fact kept one row per value, so it is read beside the
        // child's rows before they are grouped: by default the stack's
        // values as one sorted list (H5), which puts every stack under
        // exactly one key and keeps a two-valued stack apart from a
        // one-valued one; with `each`, one row per value, so a stack with
        // two values counts once under each and the counts sum to more
        // than the stacks. A stack with no value falls under null.
        let mut axis_cols: Vec<String> = Vec::new();
        let mut axis_joins: Vec<String> = Vec::new();
        let mut by_sql: Vec<String> = Vec::new();
        // what each key is when it is a date (2026-10-12)
        let mut by_dated: Vec<Option<Dated>> = Vec::new();
        // Wave 7a: the keys read by an axis, by their column's name, which
        // hold each value in the form the pack stores
        let mut axis_keys: BTreeMap<String, String> = BTreeMap::new();
        // every key that cannot be read, told in the one refusal
        let mut unread: Vec<CompileError> = Vec::new();
        for (i, c) in g.by.iter().enumerate() {
            let at = format!("{path}.group.by[{i}]");
            if c.op != "axis" {
                let key = match self.expr(c, &child_terms, "ch", &at) {
                    Ok(key) => key,
                    Err(e) => {
                        unread.push(e);
                        continue;
                    }
                };
                if !key.members {
                    by_sql.push(key.sql);
                    by_dated.push(key.dated);
                    continue;
                }
                // Wave 7a: a stack, a subject or a session counts once under
                // each dataset holding a file of it or of one of its stacks,
                // as an axis read with `each` counts a stack, and under null
                // where none holds one
                let col = format!("names_g{i}");
                let mut names: Vec<String> = self
                    .ctx
                    .names
                    .datasets()
                    .into_iter()
                    .map(|d| d.name)
                    .collect();
                names.sort();
                let alias = format!("dn{i}");
                let own = key.holder.filter(|h| self.joins_datasets(*h));
                if names.is_empty() || (own.is_some() && self.dataset_names().is_none()) {
                    axis_cols.push(format!("CAST(NULL AS TEXT) AS {col}"));
                } else if own == Some(Holder::Stack) && self.one_source_each() {
                    // a stack's own datasets, one `source` row each: its
                    // holders among them, each row once
                    let (name, sources) = self.dataset_names().unwrap_or_default();
                    axis_joins.push(format!(
                        "LEFT JOIN {} {alias} ON {alias}.stack_id = ch.stack_k \
                         AND {alias}.source_id IN ({sources})",
                        self.q("source_stack")
                    ));
                    axis_cols.push(format!(
                        "{} AS {col}",
                        name.replace("dsb.", &format!("{alias}."))
                    ));
                } else if let Some((pairs, on)) =
                    own.and_then(|holder| self.dataset_pairs(holder, "ch", &alias))
                {
                    // the rows' own datasets: each row joined to its pairs
                    axis_joins.push(format!("LEFT JOIN {pairs} {alias} ON {on}"));
                    axis_cols.push(format!("{alias}.n AS {col}"));
                } else {
                    let rows = names
                        .iter()
                        .map(|n| format!("SELECT CAST('{}' AS TEXT) AS n", n.replace('\'', "''")))
                        .collect::<Vec<_>>()
                        .join(" UNION ALL ");
                    axis_joins.push(format!(
                        "LEFT JOIN ({rows}) {alias} ON {}",
                        self.sql.member(&key.sql, &format!("{alias}.n"))
                    ));
                    axis_cols.push(format!("{alias}.n AS {col}"));
                }
                by_sql.push(format!("ch.{col}"));
                by_dated.push(None);
                continue;
            }
            let child_grain = self.ask.sets.get(&g.of).map(|s| s.grain);
            if child_grain != Some(Grain::Stack) {
                return Err(err(at, "an axis groups a stack set"));
            }
            let axis = c
                .ref_name()
                .ok_or_else(|| err(&at, "an axis ref names an axis"))?;
            let values = self.axis_values(axis, &child_terms, "ch", &at);
            let col = format!("axis_g{i}");
            if c.opts.get("each").and_then(Value::as_bool).unwrap_or(false) {
                let alias = format!("ax{i}");
                axis_joins.push(format!(
                    "LEFT JOIN {} ON 1 = 1",
                    self.axis_rows(&values, &alias)
                ));
                axis_cols.push(format!(
                    "{} AS {col}",
                    self.shown_form(axis, &format!("{alias}.value"))
                ));
            } else {
                axis_cols.push(format!(
                    "{} AS {col}",
                    self.axis_list(&values, |x| self.shown_form(axis, x))
                ));
            }
            axis_keys.insert(axis.to_string(), axis.to_string());
            by_sql.push(format!("ch.{col}"));
            by_dated.push(None);
        }
        together(unread)?;
        let source = if axis_cols.is_empty() {
            format!("{} ch", cte_name(&g.of))
        } else {
            format!(
                "(SELECT ch.*, {} FROM {} ch{}) ch",
                axis_cols.join(", "),
                cte_name(&g.of),
                axis_joins
                    .iter()
                    .map(|j| format!(" {j}"))
                    .collect::<String>()
            )
        };
        for (i, (c, e)) in g.by.iter().zip(by_sql).enumerate() {
            let col = format!("g{i}");
            cols.push(format!("{e} AS {col}"));
            group_cols.push(e);
            let label = c
                .ref_name()
                .map(str::to_string)
                .unwrap_or_else(|| format!("by{i}"));
            frame.group_by.push((label, col));
        }
        // Nima's ruling after record 49's review: below detail quasi a
        // group that totals a measure, or groups a set filtered on one,
        // shows its totals and its counts only for MEASURE_K scans or more
        let k = crate::validate::MEASURE_K;
        let small = self.small_cells.contains(name);
        let held = |e: String| -> String {
            if small {
                format!("CASE WHEN COUNT(*) >= {k} THEN {e} END")
            } else {
                e
            }
        };
        cols.push(format!("{} AS _rows", held("COUNT(*)".into())));
        cols.push(format!(
            "{} AS _subjects",
            held(if child.has_subj {
                "COUNT(DISTINCT ch.subj)".into()
            } else {
                "COUNT(DISTINCT ch.k)".into()
            })
        ));
        // Record 55 K7, small counts at plain (the ruling of 2026-10-12):
        // the answer's group counts its subjects as they are, whatever a
        // measure's rule holds, so its counts are held by them
        let counted = name == self.ask.out.set && !self.counts.is_empty();
        if counted {
            cols.push(format!(
                "{} AS {COUNT_SUBJECTS}",
                if child.has_subj {
                    "COUNT(DISTINCT ch.subj)"
                } else {
                    "COUNT(DISTINCT ch.k)"
                }
            ));
        }
        frame.bindings.push(("_rows".into(), "_rows".into()));
        frame
            .bindings
            .push(("_subjects".into(), "_subjects".into()));
        let mut terms: Vec<(String, Term)> = vec![
            ("_rows".into(), Term::plain("_rows".into())),
            ("_subjects".into(), Term::plain("_subjects".into())),
        ];
        for ((label, col), dated) in frame.group_by.iter().zip(&by_dated) {
            terms.push((label.clone(), Term::plain(col.clone()).dated(*dated)));
        }
        // Record 55 K7, the ruling of 2026-10-12: beside each binding of the
        // answer's group that its subjects hold, how many subjects have the
        // value it reads, which the answer's column is held to
        let held_here: BTreeSet<String> = if name == self.ask.out.set {
            self.held.values().flatten().cloned().collect()
        } else {
            BTreeSet::new()
        };
        let mut post: Vec<(String, String)> = Vec::new();
        for (b, c) in &set.bind.0 {
            let bp = format!("{path}.bind.{b}");
            let col = column_name(b);
            let dated = if AGGREGATES.contains(&c.op.as_str()) {
                let target = c.opts.get("set").and_then(Value::as_str).unwrap_or("");
                if target != g.of {
                    return Err(err(
                        bp,
                        format!("a group aggregates its own child {}, not {target}", g.of),
                    ));
                }
                let inner = match c.args.first() {
                    Some(Arg::Clause(inner)) => Some(self.expr(inner, &child_terms, "ch", &bp)?),
                    Some(_) => return Err(err(bp, "an aggregate's argument is a clause")),
                    None => None,
                };
                // the earliest or the latest of dates is a date
                let dated = inner
                    .as_ref()
                    .and_then(|t| t.dated)
                    .filter(|_| matches!(c.op.as_str(), "min" | "max"));
                let inner = inner.map(|t| t.sql);
                cols.push(format!(
                    "{} AS {col}",
                    held(self.aggregate(&c.op, inner.as_deref(), &bp)?)
                ));
                if held_here.contains(b) {
                    let x = inner
                        .as_deref()
                        .ok_or_else(|| err(&bp, "a held aggregate reads a column"))?;
                    let count = held_count_col(b);
                    cols.push(format!("{} AS {count}", subjects_with(x, child.has_subj)));
                    frame.held_counts.insert(b.clone(), count);
                }
                // small counts at plain: the subjects a binding counts by a
                // quasi identifying field of its own
                if counted {
                    for size in &self.counts.sizes {
                        if let crate::validate::CountSize::Own { binding, nonzero } = size
                            && binding == b
                        {
                            let x = inner
                                .as_deref()
                                .ok_or_else(|| err(&bp, "a held count reads a column"))?;
                            cols.push(format!(
                                "{} AS {}",
                                subjects_counted(x, *nonzero, child.has_subj),
                                own_count_col(b)
                            ));
                        }
                    }
                }
                dated
            } else {
                post.push((b.clone(), col.clone()));
                continue;
            };
            frame.bindings.push((b.clone(), col.clone()));
            terms.push((b.clone(), Term::plain(col).dated(dated)));
        }
        let mut layer = format!(
            "SELECT {} FROM {source} GROUP BY {}",
            cols.join(", "),
            group_cols.join(", ")
        );
        for (b, col) in post {
            let c = self.key_values(set.bind.get(&b).expect("a binding"), &axis_keys);
            let e = self.expr(&c, &terms, "q", &format!("{path}.bind.{b}"))?;
            layer = format!("SELECT q.*, {} AS {col} FROM ({layer}) q", e.sql);
            frame.bindings.push((b.clone(), col.clone()));
            terms.push((b, Term::plain(col).dated(e.dated)));
        }
        // A null key (a stack with no value on an axis, a field nobody
        // filled) is numbered last on both backends, so a group's key is
        // the same number on either.
        let order: Vec<String> = frame
            .group_by
            .iter()
            .map(|(_, c)| self.sql.order_term(&format!("q.{c}"), Dir::Asc))
            .collect();
        layer = format!(
            "SELECT q.*, ROW_NUMBER() OVER (ORDER BY {}) AS k, NULL AS subj FROM ({layer}) q",
            order.join(", ")
        );
        if counted {
            let (held, flags) = self.count_flags(&layer, &frame);
            layer = held;
            frame.count_flags = flags;
        }
        if !set.where_.is_empty() {
            let mut preds = Vec::new();
            for (i, c) in set.where_.iter().enumerate() {
                let c = self.key_values(c, &axis_keys);
                preds.push(
                    self.expr(&c, &terms, "q", &format!("{path}.where[{i}]"))?
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

    /// Record 55 K7, small counts at plain (the ruling of 2026-10-12): which
    /// of the answer's groups hold their counts back, one flag column per
    /// size of the rule over the layer `layer` (whose rows carry `k` and
    /// the keys), so the answer reads 1 where a count is held.
    ///
    /// A group is held where the size counts fewer than `MEASURE_K`
    /// subjects; for a binding's own count, also where the group's other
    /// subjects are fewer, since the group's total less the count is them.
    /// Then, so that no held count is the difference between a total the
    /// ask answers elsewhere and the counts shown, every line of groups
    /// (the groups that share all keys but one; with one key, all of them)
    /// that holds a count holds one more: where the line holds one alone,
    /// or holds only counts under the bound (two groups of one each would
    /// be found from their sum), the group with the fewest subjects of
    /// those shown, the first by key on a tie. With several keys this runs
    /// a pass per key three times, so that a group held for one line is
    /// never alone in another. Every group's flags are decided over all
    /// the groups, before the group's own `where`, a page or a limit, so
    /// the rows shown never decide which are held.
    fn count_flags(&self, layer: &str, frame: &Frame) -> (String, Vec<String>) {
        let k = crate::validate::MEASURE_K;
        let all = format!("q.{COUNT_SUBJECTS}");
        let mut sizes: Vec<(String, String)> = Vec::new();
        for size in &self.counts.sizes {
            match size {
                crate::validate::CountSize::Group => {
                    sizes.push((all.clone(), format!("{all} < {k}")));
                }
                crate::validate::CountSize::Own { binding, .. } => {
                    let own = format!("q.{}", own_count_col(binding));
                    sizes.push((
                        own.clone(),
                        format!(
                            "({own} BETWEEN 1 AND {small}) OR ({all} - {own} BETWEEN 1 AND {small})",
                            small = k - 1
                        ),
                    ));
                }
            }
        }
        let mut cols = Vec::new();
        for (i, (_, small)) in sizes.iter().enumerate() {
            cols.push(format!("CASE WHEN {small} THEN 1 ELSE 0 END AS kp{i}"));
            cols.push(format!("CASE WHEN {small} THEN 1 ELSE 0 END AS kh{i}_0"));
        }
        let mut layer = format!("SELECT q.*, {} FROM ({layer}) q", cols.join(", "));
        let keys: Vec<String> = frame
            .group_by
            .iter()
            .map(|(_, c)| format!("q.{c}"))
            .collect();
        let passes = if keys.len() <= 1 { 1 } else { 3 * keys.len() };
        for pass in 1..=passes {
            let mut cols = Vec::new();
            for (i, (size, _)) in sizes.iter().enumerate() {
                let h = format!("q.kh{i}_{}", pass - 1);
                let lines: Vec<String> = (0..keys.len().max(1))
                    .map(|j| {
                        let others: Vec<&str> = keys
                            .iter()
                            .enumerate()
                            .filter(|(x, _)| *x != j)
                            .map(|(_, c)| c.as_str())
                            .collect();
                        let part = if others.is_empty() {
                            String::new()
                        } else {
                            format!("PARTITION BY {} ", others.join(", "))
                        };
                        format!(
                            "(SUM({h}) OVER ({part}) >= 1 AND (SUM({h}) OVER ({part}) = 1 OR SUM({h} * (1 - q.kp{i})) OVER ({part}) = 0) AND ROW_NUMBER() OVER ({part}ORDER BY {h}, CASE WHEN {size} = 0 THEN 1 ELSE 0 END, {size}, q.k) = 1)"
                        )
                    })
                    .collect();
                cols.push(format!(
                    "CASE WHEN {h} = 1 OR {} THEN 1 ELSE 0 END AS kh{i}_{pass}",
                    lines.join(" OR ")
                ));
            }
            layer = format!("SELECT q.*, {} FROM ({layer}) q", cols.join(", "));
        }
        let flags = (0..sizes.len())
            .map(|i| format!("kh{i}_{passes}"))
            .collect();
        (layer, flags)
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
                        && !t.sql.starts_with("x_")
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
            members: t.members,
            // the spine the holder is read by is this alias's
            holder: t.holder.filter(|_| !alias.is_empty()),
            dated: t.dated,
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
            members: false,
            holder: None,
            dated: (ty == Type::Date).then_some(Dated::Day),
        })
    }

    fn arg(&mut self, a: &Arg, terms: &[(String, Term)], alias: &str, at: &str) -> R<Term> {
        match a {
            Arg::Clause(c) => self.expr(c, terms, alias, at),
            other => self.literal(other, at),
        }
    }

    /// The text an argument writes where a date may meet it: a literal, or
    /// a parameter that holds text or a date. None for anything else.
    fn loose_text(&self, a: &Arg) -> Option<String> {
        match a {
            Arg::Text(t) => Some(t.clone()),
            Arg::Clause(c) if c.op == "param" => {
                let decl = self.ask.params.get(c.ref_name()?)?;
                match (decl.type_, decl.value.as_ref()?) {
                    (ParamType::Text | ParamType::Date, Value::String(s)) => Some(s.clone()),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// A text read as a date where it meets one (2026-10-12): Postgres
    /// compares no date with text, and answered `first >= "2020-01-01"`
    /// with 500. Bound as a day, or as an instant where it writes one and
    /// meets an instant. A text that writes no date is refused here, where
    /// validate could not tell the date it meets.
    fn written_date(&mut self, text: &str, kind: Dated, at: &str) -> R<Term> {
        let w = crate::validate::written_date(text).ok_or_else(|| {
            err(
                at,
                format!("{text} is not a date; {}", crate::validate::DATE_FORMS),
            )
        })?;
        let (v, ty, dated) = match (kind, &w) {
            (Dated::Instant, WrittenDate::Instant(t)) => {
                (t.clone(), Type::Timestamp, Dated::Instant)
            }
            _ => (w.day().to_string(), Type::Date, Dated::Day),
        };
        let param = Param::from(v.as_str());
        let sql = self.p(param.clone(), ty);
        Ok(Term {
            sql,
            prec: None,
            ci: None,
            param: Some((param, ty)),
            members: false,
            holder: None,
            dated: Some(dated),
        })
    }

    /// A date read as the day it falls on: an instant's day in UTC, a day as
    /// it is.
    fn as_day(&self, t: Term) -> Term {
        match t.dated {
            Some(Dated::Instant) => Term {
                sql: self.sql.day_of(&t.sql),
                prec: None,
                ci: None,
                param: None,
                members: false,
                holder: None,
                dated: Some(Dated::Day),
            },
            _ => t,
        }
    }

    /// The two sides of a comparison (2026-10-12): a text either side
    /// writes, as a literal or a parameter, is read as a date where the
    /// other side is one, a number there is refused, and a day and an
    /// instant are compared by their days, on both backends alike.
    fn operands(
        &mut self,
        l: &Arg,
        r: &Arg,
        terms: &[(String, Term)],
        alias: &str,
        at: &str,
    ) -> R<(Term, Term)> {
        let (a, b) = match (self.loose_text(l), self.loose_text(r)) {
            (Some(text), None) => {
                let b = self.arg(r, terms, alias, at)?;
                let a = match b.dated {
                    Some(k) => self.written_date(&text, k, at)?,
                    None => self.arg(l, terms, alias, at)?,
                };
                (a, b)
            }
            (None, Some(text)) => {
                let a = self.arg(l, terms, alias, at)?;
                let b = match a.dated {
                    Some(k) => self.written_date(&text, k, at)?,
                    None => self.arg(r, terms, alias, at)?,
                };
                (a, b)
            }
            _ => {
                let a = self.arg(l, terms, alias, at)?;
                let b = self.arg(r, terms, alias, at)?;
                for (t, other) in [(&a, r), (&b, l)] {
                    if t.dated.is_some()
                        && let Some(v) = not_a_date(other)
                    {
                        return Err(err(
                            at,
                            format!("{v} is not a date; {}", crate::validate::DATE_FORMS),
                        ));
                    }
                }
                (a, b)
            }
        };
        Ok(match (a.dated, b.dated) {
            (Some(Dated::Day), Some(Dated::Instant)) => {
                let b = self.as_day(b);
                (a, b)
            }
            (Some(Dated::Instant), Some(Dated::Day)) => (self.as_day(a), b),
            _ => (a, b),
        })
    }

    /// The date a function reads (2026-10-12): a text it is given, as a
    /// literal or a parameter, is read as a day, and an instant as the day
    /// it falls on, so `days_between`, `age_at`, `shift`, `bucket` and
    /// `part` count days on both backends.
    fn date_arg(&mut self, a: &Arg, terms: &[(String, Term)], alias: &str, at: &str) -> R<Term> {
        if let Some(text) = self.loose_text(a) {
            return self.written_date(&text, Dated::Day, at);
        }
        if let Some(v) = not_a_date(a) {
            return Err(err(
                at,
                format!("{v} is not a date; {}", crate::validate::DATE_FORMS),
            ));
        }
        let t = self.arg(a, terms, alias, at)?;
        Ok(self.as_day(t))
    }

    /// The values one expression may yield, a coalesce's, a case's
    /// branches, a least's or a greatest's (2026-10-12): where one is a
    /// date, a text among them is read as a date, and all are instants
    /// where every date among them is one, else days, so Postgres finds
    /// one type for them. Each value's text, and what they are.
    fn alike(
        &mut self,
        args: &[&Arg],
        terms: &[(String, Term)],
        alias: &str,
        at: &str,
    ) -> R<(Vec<String>, Option<Dated>)> {
        let mut compiled: Vec<Option<Term>> = Vec::with_capacity(args.len());
        for a in args {
            compiled.push(match self.loose_text(a) {
                Some(_) => None,
                None => Some(self.arg(a, terms, alias, at)?),
            });
        }
        let mut kinds: Vec<Dated> = compiled.iter().flatten().filter_map(|t| t.dated).collect();
        if !kinds.is_empty() {
            for a in args {
                if let Some(w) = self
                    .loose_text(a)
                    .and_then(|t| crate::validate::written_date(&t))
                {
                    kinds.push(match w {
                        WrittenDate::Day(_) => Dated::Day,
                        WrittenDate::Instant(_) => Dated::Instant,
                    });
                }
            }
        }
        let kind = match kinds.as_slice() {
            [] => None,
            k if k.iter().all(|d| *d == Dated::Instant) => Some(Dated::Instant),
            _ => Some(Dated::Day),
        };
        let mut out = Vec::with_capacity(args.len());
        for (a, t) in args.iter().zip(compiled) {
            out.push(match (t, kind) {
                (Some(t), Some(Dated::Day)) => self.as_day(t).sql,
                (Some(t), _) => t.sql,
                (None, Some(k)) => {
                    let text = self.loose_text(a).unwrap_or_default();
                    self.written_date(&text, k, at)?.sql
                }
                (None, None) => self.arg(a, terms, alias, at)?.sql,
            });
        }
        Ok((out, kind))
    }

    /// A long list of dates, bound as one JSON array as [`Self::long_list`]
    /// binds one, each read back as a date of the type it is compared as.
    fn long_dates(&mut self, values: Vec<String>, ty: Type) -> String {
        let doc = Value::Array(values.into_iter().map(Value::from).collect()).to_string();
        let ph = self.p(Param::Text(doc), Type::Text);
        match self.ctx.dialect {
            Dialect::Sqlite => format!("(SELECT value FROM json_each({ph}))"),
            _ => format!(
                "(SELECT (e.v)::{} FROM jsonb_array_elements_text(({ph})::jsonb) AS e(v))",
                if ty == Type::Timestamp {
                    "timestamptz"
                } else {
                    "date"
                }
            ),
        }
    }

    /// The literals of a list, or of a list parameter's value, as
    /// parameters, not yet bound.
    fn list_params(&self, a: &Arg, at: &str) -> R<Vec<Param>> {
        Ok(match a {
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
        })
    }

    /// One item of a list, bound as its own placeholder.
    fn item_placeholder(&mut self, i: Param) -> String {
        let ty = match &i {
            Param::Int(_) => Type::Int,
            Param::Double(_) => Type::Double,
            _ => Type::Text,
        };
        self.p(i, ty)
    }

    fn list_placeholders(&mut self, a: &Arg, at: &str) -> R<String> {
        let items = self.list_params(a, at)?;
        if items.is_empty() {
            return Ok("(NULL)".into());
        }
        if items.len() > LIST_BIND_MAX {
            return self.long_list(items, at);
        }
        let mut ph = Vec::with_capacity(items.len());
        for i in items {
            ph.push(self.item_placeholder(i));
        }
        Ok(format!("({})", ph.join(", ")))
    }

    /// A long list, bound as one JSON array and read back as rows: a
    /// placeholder per item stops at the backends' limits (65,535 bound
    /// values on Postgres, 32,766 on SQLite), and a selection of every
    /// stack lists hundreds of thousands. The items are of one kind, as
    /// Postgres compares them against one type.
    fn long_list(&mut self, items: Vec<Param>, at: &str) -> R<String> {
        let ints = items.iter().all(|i| matches!(i, Param::Int(_)));
        let numbers = items
            .iter()
            .all(|i| matches!(i, Param::Int(_) | Param::Double(_)));
        let texts = items.iter().all(|i| matches!(i, Param::Text(_)));
        if !(numbers || texts) {
            return Err(err(
                at,
                format!(
                    "a list of more than {LIST_BIND_MAX} values holds one kind: numbers or texts"
                ),
            ));
        }
        let array: Vec<Value> = items
            .into_iter()
            .map(|i| match i {
                Param::Int(n) => Value::from(n),
                Param::Double(d) => Value::from(d),
                Param::Text(t) => Value::from(t),
                _ => Value::Null,
            })
            .collect();
        let doc = Value::Array(array).to_string();
        let ph = self.p(Param::Text(doc), Type::Text);
        Ok(match self.ctx.dialect {
            Dialect::Sqlite => format!("(SELECT value FROM json_each({ph}))"),
            _ => {
                let cast = if ints {
                    "bigint"
                } else if numbers {
                    "double precision"
                } else {
                    "text"
                };
                format!(
                    "(SELECT (e.v)::{cast} FROM jsonb_array_elements_text(({ph})::jsonb) AS e(v))"
                )
            }
        })
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
                    members: false,
                    holder: None,
                    dated: Some(Dated::Day),
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
                let mut parts: Vec<String> = Vec::new();
                for m in &spec.exact {
                    parts.push(self.signature_member(m, None, terms, alias, at)?);
                }
                for (m, step) in &spec.rounded {
                    parts.push(self.signature_member(m, Some(*step), terms, alias, at)?);
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

    /// A row's value of an axis read in the one form the pack stores (Wave
    /// 7a): every other name a row may hold for a value (its identity where
    /// the pack stores the label, an identity it had before a rename) reads
    /// as the stored one, so a group's key and a level's signature hold each
    /// value once, whichever form its rows hold.
    fn stored_form(&self, axis: &str, x: &str) -> String {
        let synonyms = self
            .ctx
            .names
            .axis_names(axis)
            .map(|a| a.synonyms())
            .unwrap_or_default();
        Self::one_form(x, &synonyms)
    }

    /// A row's value of an axis read in the word a person reads (record 55,
    /// ruling 23): as [`Self::stored_form`], except that a value whose label
    /// changed after rows were written reads as its label, so a group's key
    /// and the value sampler say `SPGRE` where rows keep `FLASH`.
    fn shown_form(&self, axis: &str, x: &str) -> String {
        let synonyms = self
            .ctx
            .names
            .axis_names(axis)
            .map(|a| a.shown_synonyms())
            .unwrap_or_default();
        Self::one_form(x, &synonyms)
    }

    fn one_form(x: &str, synonyms: &[(String, String)]) -> String {
        if synonyms.is_empty() {
            return x.to_string();
        }
        let whens: String = synonyms
            .iter()
            .map(|(name, stored)| format!(" WHEN {} THEN {}", sql_text(name), sql_text(stored)))
            .collect();
        format!("(CASE {x}{whens} ELSE {x} END)")
    }

    /// Every text a row may hold for the value of an axis a document's text
    /// names (Wave 7a); none when it names no value of the axis.
    fn held_texts(&self, axis: &str, text: &str) -> Option<Vec<String>> {
        let names = self.ctx.names.axis_names(axis)?;
        names.named(text).map(|v| names.held(v))
    }

    /// What a row's value of an axis (`ax.value`) is held against when a
    /// document names one (Wave 7a): every text a row may hold for the value
    /// named, by its identity, an identity it had before a rename or its
    /// label, so the value is found whichever form its rows hold. A value
    /// is always text, so a number a document wrote is read as its digits;
    /// a text that names no value is compared as written, and a value an
    /// expression works out is compared as it comes.
    fn axis_held(
        &mut self,
        axis: &str,
        value: &Arg,
        terms: &[(String, Term)],
        alias: &str,
        at: &str,
    ) -> R<Held> {
        let text = match value {
            Arg::Text(t) => Some(t.clone()),
            Arg::Int(n) => Some(n.to_string()),
            Arg::Number(n) => Some(n.to_string()),
            Arg::Clause(c) if c.op == "param" => c
                .ref_name()
                .and_then(|n| self.ask.params.get(n))
                .and_then(|d| match (d.type_, d.value.as_ref()) {
                    (ParamType::Text, Some(Value::String(s))) => Some(s.clone()),
                    (ParamType::Integer, Some(Value::Number(n))) if n.is_i64() => {
                        Some(n.to_string())
                    }
                    _ => None,
                }),
            _ => None,
        };
        let Some(text) = text else {
            let v = self.arg(value, terms, alias, at)?;
            return Ok(Held::Sql(format!("ax.value = {}", v.sql)));
        };
        let held = self
            .held_texts(axis, &text)
            .unwrap_or_else(|| vec![text.clone()]);
        Ok(Held::Any(
            held.into_iter()
                .map(|h| self.p(Param::from(h), Type::Text))
                .collect(),
        ))
    }

    /// A clause over a group's keys with each value it names of a key read
    /// by an axis written as the key holds it (Wave 7a): `=`, `<>`, `in` and
    /// `not_in` on such a key compare with the name a person reads, which
    /// is the key's own form, whichever name of the value the document
    /// wrote.
    fn key_values(&self, c: &Clause, keys: &BTreeMap<String, String>) -> Clause {
        let mut out = c.clone();
        if matches!(c.op.as_str(), "=" | "<>" | "in" | "not_in")
            && let Some(Arg::Clause(l)) = c.args.first()
            && l.op == "field"
            && let Some(axis) = l.ref_name().and_then(|p| keys.get(p))
            && let Some(names) = self.ctx.names.axis_names(axis)
        {
            let stored = |a: &Arg| -> Option<Arg> {
                let text = match a {
                    Arg::Text(t) => t.clone(),
                    Arg::Int(n) => n.to_string(),
                    Arg::Number(n) => n.to_string(),
                    _ => return None,
                };
                names
                    .named(&text)
                    .map(|v| Arg::Text(names.shown(v).to_string()))
            };
            match out.args.get_mut(1) {
                Some(Arg::List(items)) => {
                    for i in items.iter_mut() {
                        if let Some(s) = stored(i) {
                            *i = s;
                        }
                    }
                }
                Some(r) => {
                    if let Some(s) = stored(r) {
                        *r = s;
                    }
                }
                None => {}
            }
            return out;
        }
        for a in out.args.iter_mut() {
            if let Arg::Clause(inner) = a {
                *inner = self.key_values(inner, keys);
            }
        }
        out
    }

    /// A derived field held against a value, written over the columns it is
    /// derived from (Postgres): the acquisition type, read or filled, with
    /// `=` or `<>`, and the largest or the smallest side of the voxel with
    /// an order. The planner keeps statistics of columns and none of an
    /// expression over them: `COALESCE(filled, read) = '3D'` it takes for
    /// one row in two hundred and `GREATEST(..) <= 1.2` for one in three,
    /// whatever the columns hold, and a set it takes for a few hundred
    /// stacks where it is a hundred thousand is joined a row at a time.
    /// Each form answers what the expression answers for every row, a null
    /// where it is null: the acquisition type is the filled one where there
    /// is one, else the one read; the largest side is at most a value where
    /// every side there is is, and at least a value where one is, and null
    /// where there is no side. None for any other comparison.
    fn derived_compared(
        &mut self,
        op: &str,
        c: &Clause,
        terms: &[(String, Term)],
        alias: &str,
        at: &str,
    ) -> R<Option<String>> {
        if !self.sql.is_pg() || c.args.len() != 2 {
            return Ok(None);
        }
        let derived = |a: &Arg| match a {
            Arg::Clause(d) if d.op == "derived" => Some(d.clone()),
            _ => None,
        };
        let value = |a: &Arg| match a {
            Arg::Text(_) | Arg::Int(_) | Arg::Number(_) => true,
            Arg::Clause(p) => p.op == "param",
            _ => false,
        };
        let flipped = match op {
            "<" => ">",
            "<=" => ">=",
            ">" => "<",
            ">=" => "<=",
            same => same,
        };
        let (d, v, op) = match (derived(&c.args[0]), derived(&c.args[1])) {
            (Some(d), None) if value(&c.args[1]) => (d, &c.args[1], op),
            (None, Some(d)) if value(&c.args[0]) => (d, &c.args[0], flipped),
            _ => return Ok(None),
        };
        let col =
            |b: &Builder, path: &str| -> R<String> { Ok(b.term(terms, alias, path, at)?.sql) };
        match (d.ref_name().unwrap_or(""), op) {
            ("acquisition_type", "=" | "<>") => {
                let filled = col(self, "acquisition_type_filled")?;
                let read = col(self, "mr_acquisition_type")?;
                let v1 = self.arg(v, terms, alias, at)?.sql;
                let v2 = self.arg(v, terms, alias, at)?.sql;
                Ok(Some(format!(
                    "(({filled} IS NOT NULL AND {filled} {op} {v1}) OR ({filled} IS NULL AND {read} {op} {v2}))"
                )))
            }
            (name @ ("voxel" | "voxel_max" | "voxel_min"), "<" | "<=" | ">" | ">=") => {
                let third = match name {
                    "voxel_min" => "slice_thickness",
                    _ => d
                        .opts
                        .get("third")
                        .and_then(Value::as_str)
                        .unwrap_or("slice_thickness"),
                };
                let sides = [
                    col(self, "pixel_spacing_row")?,
                    col(self, "pixel_spacing_col")?,
                    col(self, third)?,
                ];
                // every side is within the value where the largest is at
                // most it, or the smallest at least it; one side is where
                // the largest is at least it, or the smallest at most it
                let every = (name == "voxel_min") == matches!(op, ">" | ">=");
                let mut parts = Vec::with_capacity(4);
                for side in &sides {
                    let v = self.arg(v, terms, alias, at)?.sql;
                    parts.push(if every {
                        format!("({side} IS NULL OR {side} {op} {v})")
                    } else {
                        format!("({side} IS NOT NULL AND {side} {op} {v})")
                    });
                }
                let some = format!("COALESCE({})", sides.join(", "));
                Ok(Some(if every {
                    format!("({} AND ({some} IS NOT NULL OR NULL))", parts.join(" AND "))
                } else {
                    format!("({} OR ({some} IS NULL AND NULL))", parts.join(" OR "))
                }))
            }
            _ => Ok(None),
        }
    }

    /// One member of a level's signature, as text that is the same on both
    /// backends: an axis as its sorted values, the acquisition type and the
    /// orientation as read, a physics number as an integer at a step.
    fn signature_member(
        &mut self,
        member: &str,
        step: Option<f64>,
        terms: &[(String, Term)],
        alias: &str,
        at: &str,
    ) -> R<String> {
        if self.ctx.names.axis_values(member).is_some() {
            let values = self.axis_values(member, terms, alias, at);
            return Ok(format!(
                "COALESCE({}, '')",
                self.axis_list(&values, |x| self.stored_form(member, x))
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
                "an axis is read through has or =, which compile it as a predicate, or as a group's by",
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
                    let held = self.axis_held(axis, value, terms, alias, at)?;
                    let values = self.axis_values(axis, terms, alias, at);
                    let exists = self.axis_holds(&values, &held);
                    return plain(if op == "<>" {
                        format!("NOT {exists}")
                    } else {
                        exists
                    });
                }
                if matches!(op, "=" | "<>")
                    && let Some(among) = self.dataset_among(c, terms, alias, at)
                {
                    return plain(if op == "=" {
                        among
                    } else {
                        format!("(NOT {among})")
                    });
                }
                if let Some(by_columns) = self.derived_compared(op, c, terms, alias, at)? {
                    return plain(by_columns);
                }
                let (a, b) = self.operands(
                    c.args
                        .first()
                        .ok_or_else(|| err(at, format!("{op} needs two arguments")))?,
                    c.args
                        .get(1)
                        .ok_or_else(|| err(at, format!("{op} needs two arguments")))?,
                    terms,
                    alias,
                    at,
                )?;
                // Wave 7a: a stack, a subject or a session is in a dataset
                // when the dataset holds a file of it or of one of its
                // stacks, so = and <> ask whether the name is among them
                if matches!(op, "=" | "<>") && a.members != b.members {
                    let (list, name) = if a.members { (&a, &b) } else { (&b, &a) };
                    let among = self.sql.member(&list.sql, &name.sql);
                    return plain(if op == "=" {
                        among
                    } else {
                        format!("(NOT {among})")
                    });
                }
                plain(self.compare(op, a, b, strict))
            }
            "~=" => {
                let (a, b) = self.operands(
                    c.args
                        .first()
                        .ok_or_else(|| err(at, "~= needs two arguments"))?,
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
                // two dates are near within tol days, counted alike on both
                // backends (2026-10-12)
                if a.dated.is_some() || b.dated.is_some() {
                    let (a, b) = (self.as_day(a), self.as_day(b));
                    return plain(format!(
                        "(ABS({}) <= {tol})",
                        self.sql.days_between(&a.sql, &b.sql)
                    ));
                }
                plain(format!("(ABS({} - {}) <= {tol})", a.sql, b.sql))
            }
            "in" | "not_in" => {
                if let Some(Arg::Clause(l)) = c.args.first()
                    && l.op == "axis"
                {
                    // Wave 7a: one of an axis's values, each named by any of
                    // its names and found in any form a row holds it
                    let axis = l.ref_name().unwrap_or("");
                    let listed = c.args.get(1).ok_or_else(|| err(at, "in needs a list"))?;
                    let mut held: Vec<Param> = Vec::new();
                    for item in self.list_params(listed, at)? {
                        let text = match item {
                            Param::Text(t) => t,
                            Param::Int(n) => n.to_string(),
                            Param::Double(n) => n.to_string(),
                            _ => continue,
                        };
                        let texts = self
                            .held_texts(axis, &text)
                            .unwrap_or_else(|| vec![text.clone()]);
                        for t in texts {
                            let p = Param::from(t);
                            if !held.contains(&p) {
                                held.push(p);
                            }
                        }
                    }
                    let held = if held.len() > LIST_BIND_MAX {
                        Held::Sql(format!("ax.value IN {}", self.long_list(held, at)?))
                    } else {
                        Held::Any(held.into_iter().map(|h| self.item_placeholder(h)).collect())
                    };
                    let values = self.axis_values(axis, terms, alias, at);
                    let exists = self.axis_holds(&values, &held);
                    return plain(if op == "in" {
                        exists
                    } else {
                        format!("NOT {exists}")
                    });
                }
                if let Some(among) = self.dataset_among(c, terms, alias, at) {
                    return plain(if op == "in" {
                        among
                    } else {
                        format!("(NOT {among})")
                    });
                }
                let a = self.arg(
                    c.args
                        .first()
                        .ok_or_else(|| err(at, "in needs a value and a list"))?,
                    terms,
                    alias,
                    at,
                )?;
                let listed = c.args.get(1).ok_or_else(|| err(at, "in needs a list"))?;
                if a.members {
                    // Wave 7a: one of the names among a stack's, a
                    // subject's or a session's datasets
                    let items = self.list_params(listed, at)?;
                    if items.len() > LIST_BIND_MAX {
                        return Err(err(
                            at,
                            format!(
                                "a list of more than {LIST_BIND_MAX} datasets is not one to ask for"
                            ),
                        ));
                    }
                    let mut among = Vec::with_capacity(items.len());
                    for i in items {
                        let ph = self.item_placeholder(i);
                        among.push(self.sql.member(&a.sql, &ph));
                    }
                    let any = if among.is_empty() {
                        "(1 = 0)".to_string()
                    } else {
                        format!("({})", among.join(" OR "))
                    };
                    return plain(if op == "in" {
                        any
                    } else {
                        format!("(NOT {any})")
                    });
                }
                if let Some(kind) = a.dated {
                    // dates listed against a date (2026-10-12): each read as
                    // a date, instants against an instant where every one is
                    // an instant, else the days
                    let mut written = Vec::new();
                    for i in self.list_params(listed, at)? {
                        let w = match &i {
                            Param::Text(t) => crate::validate::written_date(t),
                            _ => None,
                        };
                        written.push(w.ok_or_else(|| {
                            let v = match i {
                                Param::Text(t) => t,
                                Param::Int(n) => n.to_string(),
                                Param::Double(d) => d.to_string(),
                                _ => "a value".to_string(),
                            };
                            err(
                                at,
                                format!("{v} is not a date; {}", crate::validate::DATE_FORMS),
                            )
                        })?);
                    }
                    let instants = kind == Dated::Instant
                        && written.iter().all(|w| matches!(w, WrittenDate::Instant(_)));
                    let (lhs, ty) = if instants {
                        (a.sql, Type::Timestamp)
                    } else {
                        (self.as_day(a).sql, Type::Date)
                    };
                    let values: Vec<String> = written
                        .iter()
                        .map(|w| match w {
                            WrittenDate::Instant(t) if instants => t.clone(),
                            other => other.day().to_string(),
                        })
                        .collect();
                    let list = if values.is_empty() {
                        "(NULL)".to_string()
                    } else if values.len() > LIST_BIND_MAX {
                        self.long_dates(values, ty)
                    } else {
                        let ph: Vec<String> = values
                            .into_iter()
                            .map(|v| self.p(Param::from(v.as_str()), ty))
                            .collect();
                        format!("({})", ph.join(", "))
                    };
                    return plain(format!(
                        "({lhs} {}IN {list})",
                        if op == "not_in" { "NOT " } else { "" }
                    ));
                }
                let list = self.list_placeholders(listed, at)?;
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
                    if let Some(among) = self.dataset_among(c, terms, alias, at) {
                        return plain(among);
                    }
                    // Wave 7a: the datasets of a stack, a subject or a
                    // session are many names, read as an axis of many values
                    // is
                    let list = self.expr(l, terms, alias, at)?;
                    if !list.members {
                        return Err(err(
                            at,
                            "has takes an axis, or a field of several names: a stack's, a subject's or a session's dataset",
                        ));
                    }
                    let v = self.arg(
                        c.args.get(1).ok_or_else(|| err(at, "has needs a value"))?,
                        terms,
                        alias,
                        at,
                    )?;
                    return plain(self.sql.member(&list.sql, &v.sql));
                }
                let axis = l.ref_name().unwrap_or("");
                let held = self.axis_held(
                    axis,
                    c.args.get(1).ok_or_else(|| err(at, "has needs a value"))?,
                    terms,
                    alias,
                    at,
                )?;
                let values = self.axis_values(axis, terms, alias, at);
                plain(self.axis_holds(&values, &held))
            }
            "picked" => {
                let role = self
                    .opt_placeholder(&c.opts, "role", at)?
                    .ok_or_else(|| err(at, "picked names its role: {role}"))?;
                let digest = self.p(Param::from(self.ctx.scheme_digest.as_str()), Type::Text);
                let picks = self.main_picks(Some(&c.opts), at)?;
                let mut sql = format!(
                    "EXISTS (SELECT 1 FROM {picks} p JOIN {} ps ON ps.pick_id = p.id WHERE ps.stack_id = {} AND p.role = {role} \
                     AND p.scheme_digest = {digest}",
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
                // a row's own datasets: none where no dataset holds a file
                // of its stacks, asked of the holders, never by making the
                // list for every row
                if let Some(among) = self.dataset_among(c, terms, alias, at) {
                    return plain(if op == "not_null" {
                        among
                    } else {
                        format!("(NOT {among})")
                    });
                }
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
                let args: Vec<&Arg> = c.args.iter().collect();
                let (parts, dated) = self.alike(&args, terms, alias, at)?;
                Ok(Term::plain(format!("COALESCE({})", parts.join(", "))).dated(dated))
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
                let then = c.args.get(1).ok_or_else(|| err(at, "case takes a then"))?;
                let branches: Vec<&Arg> = std::iter::once(then).chain(c.args.get(2)).collect();
                let (values, dated) = self.alike(&branches, terms, alias, at)?;
                let otherwise = values.get(1).cloned().unwrap_or_else(|| "NULL".into());
                Ok(Term::plain(format!(
                    "CASE WHEN {} THEN {} ELSE {otherwise} END",
                    cond.sql, values[0]
                ))
                .dated(dated))
            }
            "concat" => {
                let mut parts = Vec::new();
                for a in &c.args {
                    parts.push(self.arg(a, terms, alias, at)?.sql);
                }
                plain(format!("({})", parts.join(" || ")))
            }
            "days_between" => {
                let a = self.date_arg(
                    c.args
                        .first()
                        .ok_or_else(|| err(at, "days_between needs two dates"))?,
                    terms,
                    alias,
                    at,
                )?;
                let b = self.date_arg(
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
                let d = self.date_arg(
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
                Ok(Term::plain(self.sql.shift_days(&d.sql, &days)).dated(Some(Dated::Day)))
            }
            "age_at" => {
                let birth = self.date_arg(
                    c.args
                        .first()
                        .ok_or_else(|| err(at, "age_at needs a birth date and a day"))?,
                    terms,
                    alias,
                    at,
                )?;
                let day = self.date_arg(
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
                let d = self.date_arg(
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
                // a unit the dialect does not read is refused at the
                // clause's path, not the op's name
                plain(
                    if op == "bucket" {
                        self.sql.bucket(&d.sql, unit)
                    } else {
                        self.sql.part(&d.sql, unit)
                    }
                    .map_err(|e| err(at, e.message))?,
                )
            }
            "least" | "greatest" => {
                let a = c
                    .args
                    .first()
                    .ok_or_else(|| err(at, format!("{op} needs two arguments")))?;
                let b = c
                    .args
                    .get(1)
                    .ok_or_else(|| err(at, format!("{op} needs two arguments")))?;
                let (v, dated) = self.alike(&[a, b], terms, alias, at)?;
                Ok(Term::plain(if op == "least" {
                    self.sql.least(&v[0], &v[1])
                } else {
                    self.sql.greatest(&v[0], &v[1])
                })
                .dated(dated))
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
                    members: false,
                    holder: None,
                    dated: a.dated,
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
                    Some(Arg::Clause(inner)) => Some(self.expr(inner, &child.terms, "ch", at)?),
                    Some(_) => return Err(err(at, "an aggregate's argument is a clause")),
                    None => None,
                };
                let dated = inner
                    .as_ref()
                    .and_then(|t| t.dated)
                    .filter(|_| matches!(op, "min" | "max"));
                let inner = inner.map(|t| t.sql);
                let agg = self.aggregate(op, inner.as_deref(), at)?;
                Ok(Term::plain(format!(
                    "(SELECT {agg} FROM {} ch WHERE {link})",
                    cte_name(&target)
                ))
                .dated(dated))
            }
            other => Err(err(at, format!("{other} is not an op of the language"))),
        }
    }

    /// Record 55 K7, small counts at plain (the ruling of 2026-10-12): how
    /// many subjects a count of the answer's set counts, where a quasi
    /// identifying field chose or grouped its rows; none where none did. A
    /// group set's rows are its keys' values, so its count is held by the
    /// subjects under it, as a count of distinct values is.
    fn quasi_subjects(&self, frame: &Frame) -> Option<String> {
        let out = &self.ask.out;
        if !self.quasi_sets.contains(&out.set) {
            return None;
        }
        let set = self.ask.sets.get(&out.set)?;
        Some(match &set.group {
            Some(g) => {
                let child = self.frames.get(&g.of).cloned().unwrap_or_default();
                let who = if child.has_subj { "ch.subj" } else { "ch.k" };
                format!("(SELECT COUNT(DISTINCT {who}) FROM {} ch)", cte_name(&g.of))
            }
            None if frame.has_subj => "COUNT(DISTINCT o.subj)".into(),
            None => "COUNT(*)".into(),
        })
    }

    /// Record 55 K7, small lists at plain (the ruling of 2026-10-13): how
    /// many subjects a list of the answer's set would show, as one value
    /// read apart from the list, where a quasi identifying field chose or
    /// grouped its rows; none where none did. A group set's rows are its
    /// keys' values, so its list is held by the subjects under it.
    fn list_subjects(&self, frame: &Frame) -> Option<String> {
        let out = &self.ask.out;
        if !self.quasi_sets.contains(&out.set) {
            return None;
        }
        let set = self.ask.sets.get(&out.set)?;
        if set.group.is_some() {
            return self.quasi_subjects(frame);
        }
        let who = if frame.has_subj {
            "COUNT(DISTINCT x.subj)"
        } else {
            "COUNT(*)"
        };
        Some(format!("(SELECT {who} FROM {} x)", cte_name(&out.set)))
    }

    /// The final SELECT over the answer's set.
    fn answer(&mut self) -> R<String> {
        let out = &self.ask.out;
        let frame = self.frames.get(&out.set).cloned().unwrap_or_default();
        let terms = frame.terms.clone();
        let mut cols = vec!["o.k AS _key".to_string(), "o.subj AS _subject".to_string()];
        let mut names = vec!["_key".to_string(), "_subject".to_string()];
        let mut code_columns = Vec::new();
        let mut shaped_columns = Vec::new();
        let mut held_columns = Vec::new();
        let mut count_columns = Vec::new();
        match out.level {
            crate::ast::Level::Count => {
                let subjects = if frame.has_subj {
                    "COUNT(DISTINCT o.subj)"
                } else {
                    "COUNT(*)"
                };
                // a count of a set filtered on a measure, below detail
                // quasi: none, or MEASURE_K and more, else withheld
                let held = |e: String| -> String {
                    if self.small_cells.contains(&out.set) {
                        format!(
                            "CASE WHEN COUNT(*) = 0 OR COUNT(*) >= {} THEN {e} END",
                            crate::validate::MEASURE_K
                        )
                    } else {
                        e
                    }
                };
                // record 55 K7, small counts at plain (the ruling of
                // 2026-10-12): a count of a set a quasi identifying field
                // chose or grouped, none or MEASURE_K subjects and more,
                // else held back
                let quasi = self.quasi_subjects(&frame);
                let held_q = |e: String| -> String {
                    match &quasi {
                        Some(n) => format!(
                            "CASE WHEN {n} = 0 OR {n} >= {} THEN {e} END",
                            crate::validate::MEASURE_K
                        ),
                        None => e,
                    }
                };
                let sql = format!(
                    "SELECT {} AS rows_, {} AS subjects_ FROM {} o",
                    held_q(held(self.sql.as_bigint("COUNT(*)"))),
                    held_q(held(self.sql.as_bigint(subjects))),
                    cte_name(&out.set)
                );
                self.answer_columns = vec!["rows".into(), "subjects".into()];
                if quasi.is_some() {
                    self.count_columns = vec![0, 1];
                }
                return Ok(sql);
            }
            crate::ast::Level::Boolean => {
                // record 55 K7, small counts at plain: whether any is a
                // count too, so it answers none, MEASURE_K subjects and
                // more, or is held back between
                if let Some(n) = self.quasi_subjects(&frame) {
                    let k = crate::validate::MEASURE_K;
                    let measured = if self.small_cells.contains(&out.set) {
                        format!(" AND COUNT(*) >= {k}")
                    } else {
                        String::new()
                    };
                    let sql = format!(
                        "SELECT CASE WHEN COUNT(*) = 0 THEN 0 WHEN {n} >= {k}{measured} THEN 1 END AS any_ FROM {} o",
                        cte_name(&out.set)
                    );
                    self.answer_columns = vec!["any".into()];
                    self.count_columns = vec![0];
                    return Ok(sql);
                }
                let sql = if self.small_cells.contains(&out.set) {
                    format!(
                        "SELECT CASE WHEN COUNT(*) = 0 THEN 0 WHEN COUNT(*) >= {} THEN 1 END AS any_ FROM {} o",
                        crate::validate::MEASURE_K,
                        cte_name(&out.set)
                    )
                } else {
                    format!(
                        "SELECT CASE WHEN EXISTS (SELECT 1 FROM {} o) THEN 1 ELSE 0 END AS any_",
                        cte_name(&out.set)
                    )
                };
                self.answer_columns = vec!["any".into()];
                return Ok(sql);
            }
            _ => {}
        }
        // record 55 K7, small lists at plain (the ruling of 2026-10-13): a
        // list of the rows a quasi identifying field chose is a count by
        // its length, so where the set holds 1 to 4 subjects no row of it
        // is answered. The guard reads the whole set, before a page, an
        // `after` or a limit, and binds only the sets' own parameters.
        if let Some(n) = self.list_subjects(&frame) {
            self.list_guard_params = self.params.len();
            self.list_guard = Some(format!(
                "SELECT CASE WHEN {n} BETWEEN 1 AND {} THEN 1 ELSE 0 END AS held_",
                crate::validate::MEASURE_K - 1
            ));
        }
        // every column and order key that cannot be read, told in the one
        // refusal
        let mut unread: Vec<CompileError> = Vec::new();
        for (i, c) in out.columns.iter().enumerate() {
            let at = format!("out.columns[{i}]");
            let e = match self.expr(c, &terms, "o", &at) {
                Ok(e) => e,
                Err(e) => {
                    unread.push(e);
                    continue;
                }
            };
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
            let mut rendered = if is_date {
                format!("CAST({} AS TEXT)", e.sql)
            } else {
                e.sql
            };
            if name == "code" || name == "subject.code" {
                code_columns.push(cols.len());
            }
            if self.shaped.contains(&i) {
                shaped_columns.push(cols.len());
            }
            // record 55 K7, the ruling of 2026-10-12: a column made from a
            // group's mean or count of distinct values of a quasi
            // identifying field shows only where the group holds MEASURE_K
            // subjects with a value, and is null where it is held back
            if let Some(by) = self.held.get(&i) {
                let mut holds = Vec::new();
                for b in by {
                    let count = frame
                        .held_counts
                        .get(b)
                        .ok_or_else(|| err(&at, format!("no count of the subjects {b} reads")))?;
                    holds.push(format!("o.{count} >= {}", crate::validate::MEASURE_K));
                }
                if !holds.is_empty() {
                    rendered = format!("CASE WHEN {} THEN {rendered} END", holds.join(" AND "));
                }
                held_columns.push(cols.len());
            }
            // record 55 K7, small counts at plain (the ruling of
            // 2026-10-12): a count of the groups' rows a quasi identifying
            // field chose or keyed, and what is made of it, shows only where
            // no flag of its sizes holds it back
            if let Some(sizes) = self.counts.columns.get(&i) {
                let mut holds = Vec::new();
                for &s in sizes {
                    let flag = frame
                        .count_flags
                        .get(s)
                        .ok_or_else(|| err(&at, "no flag holds this count"))?;
                    holds.push(format!("o.{flag} = 0"));
                }
                rendered = format!("CASE WHEN {} THEN {rendered} END", holds.join(" AND "));
                count_columns.push(cols.len());
            }
            cols.push(format!("{rendered} AS c{i}"));
            names.push(name);
        }
        let mut ord_cols = Vec::new();
        let mut order = Vec::new();
        for (i, o) in out.order.iter().enumerate() {
            let e = match self.expr(&o.0, &terms, "o", &format!("out.order[{i}]")) {
                Ok(e) => e,
                Err(e) => {
                    unread.push(e);
                    continue;
                }
            };
            ord_cols.push(format!("{} AS ord_{i}", e.sql));
            order.push(self.sql.order_term(&format!("z.ord_{i}"), o.1));
        }
        together(unread)?;
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
        self.shaped_columns = shaped_columns;
        self.held_columns = held_columns;
        self.count_columns = count_columns;
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
        shaped: validated.shaped.clone(),
        shaped_columns: Vec::new(),
        small_cells: validated.small_cells.clone(),
        held: validated.held.clone(),
        held_columns: Vec::new(),
        quasi_sets: validated.quasi_sets.clone(),
        counts: validated.counts.clone(),
        count_columns: Vec::new(),
        list_guard: None,
        list_guard_params: 0,
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
        shaped_columns: b.shaped_columns.clone(),
        held_columns: b.held_columns.clone(),
        count_columns: b.count_columns.clone(),
        list_guard: b
            .list_guard
            .as_ref()
            .map(|g| format!("WITH {} {g}", b.ctes.join(", "))),
        list_guard_params: b.list_guard_params,
    })
}
