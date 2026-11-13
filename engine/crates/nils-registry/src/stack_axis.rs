// SPDX-License-Identifier: AGPL-3.0-only

//! One row a stack (`stack_axis`): what a question reads of every stack of
//! a set, kept beside each other so that the set is one read of one table.
//!
//! **The axes**, a read model of `classification_axis`, which keeps a row
//! per value and stays the place of truth. A question that reads an axis of
//! every stack of a set (a predicate, a group's key, a level's signature,
//! the rule that a stack the pack ruled out is in no set) read
//! `classification_axis` once per stack and axis, and on a registry of half
//! a million stacks and five million axis rows the reading was most of the
//! question's time. Here each stack has:
//!
//! - `axes`, one JSON object: each axis the stack has a row of, with the
//!   array of its values, one element per row, a row with no value as null;
//!   a stack with no axis row has the empty object;
//! - `excluded`, 1 where the stack's `disposition` holds `excluded`, with a
//!   unique index over those rows alone, so the rule is one lookup in a
//!   list of the few stacks ruled out.
//!
//! **The stack's place and its hot fingerprint columns** (schema 119).
//! With the axes in one row, what was left of a plain count of stacks was
//! the joins a stack set made row by row before it could hold a clause
//! against a stack: the stack to its fingerprint and to its row here. So
//! the row also holds
//!
//! - `series_id`, the stack's own, null where no stack of this id is (a
//!   fingerprint or an axis row some writer left of a stack that is gone);
//! - `fingerprinted`, 1 where the stack has a fingerprint row, and the
//!   columns of it a question filters stacks on ([`HOT`]): the acquisition
//!   type as filled and as read, the two pixel spacings and the slice
//!   thickness, each null where the fingerprint's is or where there is
//!   none.
//!
//! A stack set is then read from here and tied to its series; the stack's
//! own table and its fingerprint are joined only where a set names another
//! column of theirs.
//!
//! The row holds nothing its sources do not. What a value is called, in
//! which order values are listed and what a stack with none reads as are
//! all decided where the row is read, as they were on the rows themselves.
//! A row that would say nothing (no stack, no fingerprint, no axis row) is
//! no row.
//!
//! **It is never stale**, by two means.
//!
//! The axes: every writer of `classification_axis` calls [`refresh`] for
//! the stacks it wrote, inside the transaction that wrote them: the sort's
//! class and disposition phases, a pass, the session pass, the synthetic
//! registries. [`refresh`] makes the whole row again from its sources, so
//! it also mends the other columns of those stacks.
//!
//! The stack's place and the fingerprint columns: the database keeps them
//! (2026-11-13). `stack` and `stack_fingerprint` each have triggers that
//! write this row in the statement that wrote theirs ([`ensure_triggers`]).
//! Their writers are many and of many kinds: the digest's bulk insert, the
//! fingerprint job's upsert, a fold, a sweep driven by a list of tables, a
//! merge that repoints every table with a subject, the synthetic
//! registries, and tests that write a row by hand. A call in each would be
//! a list somebody keeps; a trigger is the one thing every one of them
//! passes, whoever writes and however. A trigger does not see a table
//! emptied by `TRUNCATE` on SQLite (which has none) and is told of it on
//! Postgres; it does not run where it was dropped or switched off, or
//! under a session that asks for none (`session_replication_role`), and a
//! table copied by hand comes without it. [`verify`] says whether the
//! triggers are there and compares every column of every row with its
//! sources, and [`rebuild`] sets the triggers again and makes every row
//! again (`nils repair stack-axis`); the migrations that added the table
//! and the columns are a rebuild each.

use serde::{Deserialize, Serialize};

use crate::store::{Error, Store};

/// The axis and the value of a stack the pack ruled out (wave 4b, section
/// 4.4 rule 10): no set of a question sees it.
pub const RULED_OUT: (&str, &str) = ("disposition", "excluded");

/// The columns of a stack's fingerprint row this row carries: the ones a
/// question filters stacks on (the derived acquisition type and voxel).
pub const HOT: &[&str] = &[
    "acquisition_type_filled",
    "mr_acquisition_type",
    "pixel_spacing_row",
    "pixel_spacing_col",
    "slice_thickness",
];

fn list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The empty object, as the backend writes it.
fn no_axes(store: &Store) -> &'static str {
    match store {
        Store::Sqlite(_) => "'{}'",
        Store::Postgres { .. } => "'{}'::jsonb",
    }
}

/// Whether a row says nothing: of no stack, no fingerprint and no axis row.
fn says_nothing(store: &Store) -> String {
    format!(
        "series_id IS NULL AND fingerprinted = 0 AND excluded = 0 AND axes = {}",
        no_axes(store)
    )
}

/// Every stack's row as its sources say it now, `(stack_id, series_id,
/// axes, excluded, fingerprinted,` [`HOT`]`)`, for the stacks `ids` names
/// or for all. A row for every id one of the sources has: `stack`,
/// `stack_fingerprint`, `classification_axis`. The object's keys and each
/// array's values are put in one order, so that the same rows always make
/// the same document.
fn fresh(store: &Store, ids: Option<&str>) -> String {
    let axis = store.qualified("classification_axis");
    let stack = store.qualified("stack");
    let fingerprint = store.qualified("stack_fingerprint");
    let among = |column: &str| ids.map_or(String::new(), |i| format!(" WHERE {column} IN ({i})"));
    let (ruled_axis, ruled_value) = RULED_OUT;
    let excluded = format!(
        "MAX(CASE WHEN ax.axis = '{ruled_axis}' AND ax.value = '{ruled_value}' THEN 1 ELSE 0 END)"
    );
    let scope = among("ax.stack_id");
    let axes = match store {
        Store::Sqlite(_) => format!(
            "SELECT a.stack_id AS stack_id, \
             json_group_object(a.axis, json(a.vals) ORDER BY a.axis) AS axes, \
             MAX(a.excluded) AS excluded \
             FROM (SELECT ax.stack_id AS stack_id, ax.axis AS axis, \
             json_group_array(ax.value ORDER BY ax.value) AS vals, {excluded} AS excluded \
             FROM {axis} ax{scope} GROUP BY ax.stack_id, ax.axis) a GROUP BY a.stack_id"
        ),
        Store::Postgres { .. } => format!(
            "SELECT a.stack_id AS stack_id, jsonb_object_agg(a.axis, a.vals) AS axes, \
             MAX(a.excluded) AS excluded \
             FROM (SELECT ax.stack_id AS stack_id, ax.axis AS axis, \
             jsonb_agg(ax.value ORDER BY ax.value) AS vals, {excluded} AS excluded \
             FROM {axis} ax{scope} GROUP BY ax.stack_id, ax.axis) a GROUP BY a.stack_id"
        ),
    };
    let hot: String = HOT.iter().map(|c| format!(", f.{c} AS {c}")).collect();
    // The stacks to make a row for: every id one of the sources has. Where
    // the stacks are named, Postgres is given them as one list and asked
    // which of them a source has, and not the union of what each source
    // has of them: the union it takes for the sum of three guesses (9,400
    // rows for 500 stacks of ten axis rows each), and it then reads the
    // whole of `stack` to tie them, once for every 500 stacks a sort
    // writes. A refresh of 50,000 stacks on a registry of half a million
    // took 5.3 s so and takes 1.8 s by the list, where the refresh of the
    // axes alone took 1.4 s (2026-11-13). SQLite reads the union by the
    // keys either way.
    let (ids_of, has_one) = match (store, ids) {
        (Store::Postgres { .. }, Some(i)) => (
            format!("SELECT unnest(ARRAY[{i}]::bigint[]) AS stack_id"),
            " WHERE st.id IS NOT NULL OR f.stack_id IS NOT NULL OR a.stack_id IS NOT NULL",
        ),
        _ => (
            format!(
                "SELECT id AS stack_id FROM {stack}{of_stack} \
                 UNION SELECT stack_id FROM {fingerprint}{of_row} \
                 UNION SELECT ax.stack_id FROM {axis} ax{scope}",
                of_stack = among("id"),
                of_row = among("stack_id"),
            ),
            "",
        ),
    };
    format!(
        "SELECT k.stack_id AS stack_id, st.series_id AS series_id, \
         COALESCE(a.axes, {none}) AS axes, COALESCE(a.excluded, 0) AS excluded, \
         CASE WHEN f.stack_id IS NULL THEN 0 ELSE 1 END AS fingerprinted{hot} \
         FROM ({ids_of}) k \
         LEFT JOIN {stack} st ON st.id = k.stack_id \
         LEFT JOIN {fingerprint} f ON f.stack_id = k.stack_id \
         LEFT JOIN ({axes}) a ON a.stack_id = k.stack_id{has_one}",
        none = no_axes(store),
    )
}

/// The statement that reads the rows of these stacks from their sources,
/// as [`refresh`] writes them: for a test that holds what the planner
/// makes of it.
#[doc(hidden)]
pub fn fresh_rows(store: &Store, stacks: &[i64]) -> String {
    fresh(store, Some(&list(stacks)))
}

/// The columns of a row, in the order [`fresh`] gives them.
fn columns() -> String {
    let mut out = String::from("stack_id, series_id, axes, excluded, fingerprinted");
    for c in HOT {
        out.push_str(", ");
        out.push_str(c);
    }
    out
}

/// Bring the rows of these stacks up to what their sources hold for them
/// now. Every writer of `classification_axis` calls this for the stacks it
/// wrote, inside its own transaction, after its last write. The whole row
/// is made again, the stack's place and its fingerprint columns with the
/// axes; a stack none of the sources has is left with no row here.
pub fn refresh(store: &mut Store, stacks: &[i64]) -> Result<(), Error> {
    let mine = store.qualified("stack_axis");
    let columns = columns();
    // each stack once: a row is made for each id named
    let mut stacks = stacks.to_vec();
    stacks.sort_unstable();
    stacks.dedup();
    for chunk in stacks.chunks(500) {
        let ids = list(chunk);
        store.execute(
            &format!("DELETE FROM {mine} WHERE stack_id IN ({ids})"),
            &[],
        )?;
        let rows = fresh(store, Some(&ids));
        store.execute(&format!("INSERT INTO {mine} ({columns}) {rows}"), &[])?;
    }
    Ok(())
}

/// The triggers that keep a stack's place and its fingerprint columns:
/// which table each is on, and the function it runs on Postgres.
const TRIGGERS: &[(&str, &str, &str)] = &[
    (
        "stack_axis_of_stack_written",
        "stack",
        "stack_axis_of_stack",
    ),
    (
        "stack_axis_of_stack_changed",
        "stack",
        "stack_axis_of_stack",
    ),
    (
        "stack_axis_of_stack_removed",
        "stack",
        "stack_axis_of_stack",
    ),
    (
        "stack_axis_of_fingerprint_written",
        "stack_fingerprint",
        "stack_axis_of_fingerprint",
    ),
    (
        "stack_axis_of_fingerprint_changed",
        "stack_fingerprint",
        "stack_axis_of_fingerprint",
    ),
    (
        "stack_axis_of_fingerprint_removed",
        "stack_fingerprint",
        "stack_axis_of_fingerprint",
    ),
];

/// On Postgres a table can be emptied without a row being deleted: a
/// statement trigger each for that.
const EMPTIED: &[(&str, &str, &str)] = &[
    (
        "stack_axis_of_stack_emptied",
        "stack",
        "stack_axis_of_stack_emptied",
    ),
    (
        "stack_axis_of_fingerprint_emptied",
        "stack_fingerprint",
        "stack_axis_of_fingerprint_emptied",
    ),
];

/// The functions the triggers run on Postgres, each with its body as it is
/// written between the dollar quotes. Each names its tables with their
/// schema, since a trigger runs under the search path of whoever writes;
/// so the body is this registry's own, and [`triggers`] holds what the
/// database has against it to the letter.
fn functions(store: &Store) -> Vec<(&'static str, String)> {
    let mine = store.qualified("stack_axis");
    let stack = store.qualified("stack");
    let nothing = says_nothing(store);
    let none = no_axes(store);
    let hot_columns: String = HOT.iter().map(|c| format!(", {c}")).collect();
    let hot_new: String = HOT.iter().map(|c| format!(", NEW.{c}")).collect();
    let hot_set: String = HOT
        .iter()
        .map(|c| format!(", {c} = excluded.{c}"))
        .collect();
    let hot_null: String = HOT.iter().map(|c| format!(", {c} = NULL")).collect();
    vec![
        (
            "stack_axis_of_stack",
            format!(
                "\nBEGIN\n\
                   IF TG_OP = 'DELETE' OR (TG_OP = 'UPDATE' AND OLD.id IS DISTINCT FROM NEW.id) THEN\n\
                     UPDATE {mine} SET series_id = NULL WHERE stack_id = OLD.id;\n\
                     DELETE FROM {mine} WHERE stack_id = OLD.id AND {nothing};\n\
                   END IF;\n\
                   IF TG_OP <> 'DELETE' THEN\n\
                     INSERT INTO {mine} (stack_id, series_id, axes, excluded, fingerprinted)\n\
                     VALUES (NEW.id, NEW.series_id, {none}, 0, 0)\n\
                     ON CONFLICT (stack_id) DO UPDATE SET series_id = excluded.series_id;\n\
                   END IF;\n\
                   RETURN NULL;\n\
                 END\n"
            ),
        ),
        (
            "stack_axis_of_fingerprint",
            format!(
                "\nBEGIN\n\
                   IF TG_OP = 'DELETE' OR (TG_OP = 'UPDATE' AND OLD.stack_id IS DISTINCT FROM NEW.stack_id) THEN\n\
                     UPDATE {mine} SET fingerprinted = 0{hot_null} WHERE stack_id = OLD.stack_id;\n\
                     DELETE FROM {mine} WHERE stack_id = OLD.stack_id AND {nothing};\n\
                   END IF;\n\
                   IF TG_OP <> 'DELETE' THEN\n\
                     INSERT INTO {mine} (stack_id, series_id, axes, excluded, fingerprinted{hot_columns})\n\
                     VALUES (NEW.stack_id, (SELECT st.series_id FROM {stack} st WHERE st.id = NEW.stack_id),\n\
                       {none}, 0, 1{hot_new})\n\
                     ON CONFLICT (stack_id) DO UPDATE SET fingerprinted = 1{hot_set};\n\
                   END IF;\n\
                   RETURN NULL;\n\
                 END\n"
            ),
        ),
        (
            "stack_axis_of_stack_emptied",
            format!(
                "\nBEGIN\n\
                   UPDATE {mine} SET series_id = NULL WHERE series_id IS NOT NULL;\n\
                   DELETE FROM {mine} WHERE {nothing};\n\
                   RETURN NULL;\n\
                 END\n"
            ),
        ),
        (
            "stack_axis_of_fingerprint_emptied",
            format!(
                "\nBEGIN\n\
                   UPDATE {mine} SET fingerprinted = 0{hot_null} WHERE fingerprinted <> 0;\n\
                   DELETE FROM {mine} WHERE {nothing};\n\
                   RETURN NULL;\n\
                 END\n"
            ),
        ),
    ]
}

/// Set the triggers on `stack` and `stack_fingerprint` that keep this
/// table's `series_id`, `fingerprinted` and [`HOT`] columns, replacing any
/// of the same names. Each writes the row of the stack its own row names,
/// in the statement that wrote that row:
///
/// - a stack inserted, or given another series: its row here is made or
///   its `series_id` set;
/// - a stack deleted: `series_id` is null, and the row goes if it then
///   says nothing;
/// - a fingerprint inserted, or one of its columns here changed: the row
///   is made, or `fingerprinted` and the columns set;
/// - a fingerprint deleted, or moved to another stack: `fingerprinted` is
///   0 and the columns null on the stack it left, and the row goes if it
///   then says nothing.
///
/// An insert that replaces a row on a conflict runs the insert's trigger
/// with the row that stays, so the columns are that row's.
pub fn ensure_triggers(store: &mut Store) -> Result<(), Error> {
    let nothing = says_nothing(store);
    let none = no_axes(store);
    let hot_columns: String = HOT.iter().map(|c| format!(", {c}")).collect();
    let hot_new: String = HOT.iter().map(|c| format!(", NEW.{c}")).collect();
    let hot_set: String = HOT
        .iter()
        .map(|c| format!(", {c} = excluded.{c}"))
        .collect();
    let hot_null: String = HOT.iter().map(|c| format!(", {c} = NULL")).collect();
    match store {
        Store::Sqlite(_) => {
            // `IS NOT` holds for a null against a value, which `<>` does not
            let hot_differs: String = HOT
                .iter()
                .map(|c| format!(" OR OLD.{c} IS NOT NEW.{c}"))
                .collect();
            let place = "INSERT INTO stack_axis (stack_id, series_id, axes, excluded, fingerprinted) \
                 VALUES (NEW.id, NEW.series_id, '{}', 0, 0) \
                 ON CONFLICT (stack_id) DO UPDATE SET series_id = excluded.series_id;";
            let left = |old: &str| {
                format!(
                    "UPDATE stack_axis SET series_id = NULL WHERE stack_id = {old}; \
                     DELETE FROM stack_axis WHERE stack_id = {old} AND {nothing};"
                )
            };
            let print = format!(
                "INSERT INTO stack_axis (stack_id, series_id, axes, excluded, fingerprinted{hot_columns}) \
                 VALUES (NEW.stack_id, (SELECT st.series_id FROM stack st WHERE st.id = NEW.stack_id), \
                 {none}, 0, 1{hot_new}) \
                 ON CONFLICT (stack_id) DO UPDATE SET fingerprinted = 1{hot_set};"
            );
            let unprinted = |old: &str| {
                format!(
                    "UPDATE stack_axis SET fingerprinted = 0{hot_null} WHERE stack_id = {old}; \
                     DELETE FROM stack_axis WHERE stack_id = {old} AND {nothing};"
                )
            };
            for (name, _, _) in TRIGGERS {
                store.batch(&format!("DROP TRIGGER IF EXISTS {name}"))?;
            }
            store.batch(&format!(
                "CREATE TRIGGER stack_axis_of_stack_written AFTER INSERT ON stack \
                 BEGIN {place} END;\n\
                 CREATE TRIGGER stack_axis_of_stack_changed AFTER UPDATE ON stack \
                 WHEN OLD.id IS NOT NEW.id OR OLD.series_id IS NOT NEW.series_id \
                 BEGIN \
                 UPDATE stack_axis SET series_id = NULL WHERE stack_id = OLD.id AND OLD.id IS NOT NEW.id; \
                 DELETE FROM stack_axis WHERE stack_id = OLD.id AND OLD.id IS NOT NEW.id AND {nothing}; \
                 {place} END;\n\
                 CREATE TRIGGER stack_axis_of_stack_removed AFTER DELETE ON stack \
                 BEGIN {gone} END;\n\
                 CREATE TRIGGER stack_axis_of_fingerprint_written AFTER INSERT ON stack_fingerprint \
                 BEGIN {print} END;\n\
                 CREATE TRIGGER stack_axis_of_fingerprint_changed AFTER UPDATE ON stack_fingerprint \
                 WHEN OLD.stack_id IS NOT NEW.stack_id{hot_differs} \
                 BEGIN \
                 UPDATE stack_axis SET fingerprinted = 0{hot_null} \
                 WHERE stack_id = OLD.stack_id AND OLD.stack_id IS NOT NEW.stack_id; \
                 DELETE FROM stack_axis WHERE stack_id = OLD.stack_id \
                 AND OLD.stack_id IS NOT NEW.stack_id AND {nothing}; \
                 {print} END;\n\
                 CREATE TRIGGER stack_axis_of_fingerprint_removed AFTER DELETE ON stack_fingerprint \
                 BEGIN {unprint} END;",
                gone = left("OLD.id"),
                unprint = unprinted("OLD.stack_id"),
            ))
        }
        Store::Postgres { .. } => {
            let stack = store.qualified("stack");
            let fingerprint = store.qualified("stack_fingerprint");
            let function = |name: &str| store.qualified(name);
            let hot_differs: String = HOT
                .iter()
                .map(|c| format!(" OR OLD.{c} IS DISTINCT FROM NEW.{c}"))
                .collect();
            let mut sql = String::new();
            for (name, table, _) in TRIGGERS.iter().chain(EMPTIED) {
                sql.push_str(&format!(
                    "DROP TRIGGER IF EXISTS {name} ON {};\n",
                    store.qualified(table)
                ));
            }
            for (name, body) in functions(store) {
                sql.push_str(&format!(
                    "CREATE OR REPLACE FUNCTION {}() RETURNS trigger LANGUAGE plpgsql AS $body${body}$body$;\n",
                    function(name)
                ));
            }
            sql.push_str(&format!(
                "CREATE TRIGGER stack_axis_of_stack_written AFTER INSERT ON {stack} \
                 FOR EACH ROW EXECUTE FUNCTION {of_stack}();\n\
                 CREATE TRIGGER stack_axis_of_stack_changed AFTER UPDATE ON {stack} \
                 FOR EACH ROW WHEN (OLD.id IS DISTINCT FROM NEW.id OR OLD.series_id IS DISTINCT FROM NEW.series_id) \
                 EXECUTE FUNCTION {of_stack}();\n\
                 CREATE TRIGGER stack_axis_of_stack_removed AFTER DELETE ON {stack} \
                 FOR EACH ROW EXECUTE FUNCTION {of_stack}();\n\
                 CREATE TRIGGER stack_axis_of_stack_emptied AFTER TRUNCATE ON {stack} \
                 FOR EACH STATEMENT EXECUTE FUNCTION {stack_emptied}();\n\
                 CREATE TRIGGER stack_axis_of_fingerprint_written AFTER INSERT ON {fingerprint} \
                 FOR EACH ROW EXECUTE FUNCTION {of_fingerprint}();\n\
                 CREATE TRIGGER stack_axis_of_fingerprint_changed AFTER UPDATE ON {fingerprint} \
                 FOR EACH ROW WHEN (OLD.stack_id IS DISTINCT FROM NEW.stack_id{hot_differs}) \
                 EXECUTE FUNCTION {of_fingerprint}();\n\
                 CREATE TRIGGER stack_axis_of_fingerprint_removed AFTER DELETE ON {fingerprint} \
                 FOR EACH ROW EXECUTE FUNCTION {of_fingerprint}();\n\
                 CREATE TRIGGER stack_axis_of_fingerprint_emptied AFTER TRUNCATE ON {fingerprint} \
                 FOR EACH STATEMENT EXECUTE FUNCTION {fingerprint_emptied}();\n",
                of_stack = function("stack_axis_of_stack"),
                of_fingerprint = function("stack_axis_of_fingerprint"),
                stack_emptied = function("stack_axis_of_stack_emptied"),
                fingerprint_emptied = function("stack_axis_of_fingerprint_emptied"),
            ));
            store.batch(&sql)
        }
    }
}

/// What is wrong with the triggers [`ensure_triggers`] sets, by name: none
/// of the three where they keep the rows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Triggers {
    /// Not on its table at all.
    pub missing: Vec<String>,
    /// There, and switched off, or left to run for a replica's session
    /// alone (Postgres).
    pub disabled: Vec<String>,
    /// There and running a function that is not this registry's own: one
    /// of another schema, or one whose body is not the one this engine
    /// writes, which names this registry's tables with their schema
    /// (Postgres). A registry copied under another schema's name by a dump
    /// rewritten by hand has these: its triggers would write the rows of
    /// the schema it was copied from.
    pub astray: Vec<String>,
}

impl Triggers {
    /// Every trigger is there, runs for an ordinary write and writes this
    /// registry's rows.
    pub fn set(&self) -> bool {
        self.missing.is_empty() && self.disabled.is_empty() && self.astray.is_empty()
    }

    /// The three in plain words, for a person: empty where they are set.
    pub fn in_words(&self) -> String {
        let mut said = Vec::new();
        for (names, what) in [
            (&self.missing, "missing"),
            (&self.disabled, "switched off"),
            (
                &self.astray,
                "running a function that does not write this registry's rows",
            ),
        ] {
            if !names.is_empty() {
                said.push(format!("{} {what} ({})", names.len(), names.join(", ")));
            }
        }
        said.join("; ")
    }
}

/// How the triggers stand: one read of the database's own catalogue,
/// whatever the registry's size. On Postgres each trigger's function is
/// held against the body [`ensure_triggers`] would write, to the letter.
pub fn triggers(store: &mut Store) -> Result<Triggers, Error> {
    let mut out = Triggers::default();
    match store {
        Store::Sqlite(_) => {
            let there: Vec<String> = store
                .query(
                    "SELECT name FROM sqlite_master WHERE type = 'trigger' \
                     AND name LIKE 'stack!_axis!_of!_%' ESCAPE '!'",
                    &[],
                )?
                .iter()
                .map(|r| r.text(0).map(str::to_string))
                .collect::<Result<_, _>>()?;
            for (name, _, _) in TRIGGERS {
                if !there.iter().any(|t| t == name) {
                    out.missing.push(name.to_string());
                }
            }
        }
        Store::Postgres { .. } => {
            let schema = store.schema().unwrap_or("public").to_string();
            let bodies = functions(store);
            // 'O' runs for an ordinary session and 'A' for every one; 'D'
            // is switched off and 'R' runs for a replica's only
            let there = store.query(
                "SELECT t.tgname::text, c.relname::text, t.tgenabled::text, pn.nspname::text, \
                 p.proname::text, p.prosrc \
                 FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 JOIN pg_proc p ON p.oid = t.tgfoid \
                 JOIN pg_namespace pn ON pn.oid = p.pronamespace \
                 WHERE n.nspname = $1 AND NOT t.tgisinternal \
                 AND c.relname IN ('stack', 'stack_fingerprint') \
                 AND t.tgname LIKE 'stack!_axis!_of!_%' ESCAPE '!'",
                &[crate::store::Param::from(schema.clone())],
            )?;
            for (name, table, function) in TRIGGERS.iter().chain(EMPTIED) {
                let Some(row) = there.iter().find(|r| {
                    r.text(0).is_ok_and(|n| n == *name) && r.text(1).is_ok_and(|t| t == *table)
                }) else {
                    out.missing.push(name.to_string());
                    continue;
                };
                if !matches!(row.text(2)?, "O" | "A") {
                    out.disabled.push(name.to_string());
                }
                let body = bodies
                    .iter()
                    .find(|(f, _)| f == function)
                    .map(|(_, b)| b.as_str());
                if row.text(3)? != schema || row.text(4)? != *function || Some(row.text(5)?) != body
                {
                    out.astray.push(name.to_string());
                }
            }
        }
    }
    Ok(out)
}

/// What an open did where it found the triggers not set ([`mend`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mended {
    /// What it found.
    pub found: Triggers,
    /// The rows made again, where the triggers were set and the rows made;
    /// none where that failed, and then `error` says why.
    pub rows: Option<u64>,
    pub error: Option<String>,
}

/// Look for the triggers, and where one is missing, switched off or astray
/// set them all again and make every row again, in one transaction with an
/// audit row and a new epoch, as `nils repair stack-axis` records a
/// rebuild: the rows written while the triggers were not there say what
/// their sources said before. None where the triggers are set, which is
/// one read of the catalogue. A registry is opened either way: where the
/// mending fails (a store opened to read, a role that may not write the
/// table), `error` says so and nothing is changed.
pub fn mend(store: &mut Store, principal: &str) -> Result<Option<Mended>, Error> {
    let found = triggers(store)?;
    if found.set() {
        return Ok(None);
    }
    store.begin()?;
    let done = (|| -> Result<u64, Error> {
        let rows = rebuild(store)?;
        crate::home::next_epoch_in(store)?;
        crate::audit::record_in(
            store,
            &crate::audit::Entry {
                principal,
                action: crate::audit::Action::RegistryRepair,
                scope: serde_json::json!({"repair": "stack-axis", "at": "open"}),
                policy: None,
                job_id: None,
                details: Some(serde_json::json!({
                    "triggers_missing": found.missing,
                    "triggers_disabled": found.disabled,
                    "triggers_astray": found.astray,
                    "rows": rows,
                })),
            },
        )?;
        Ok(rows)
    })();
    Ok(Some(match done {
        Ok(rows) => {
            store.commit()?;
            Mended {
                found,
                rows: Some(rows),
                error: None,
            }
        }
        Err(e) => {
            store.rollback().ok();
            Mended {
                found,
                rows: None,
                error: Some(e.to_string()),
            }
        }
    }))
}

/// Set the triggers and make every row again from its sources, and say how
/// many stacks have one. Runs inside the caller's transaction, if any.
pub fn rebuild(store: &mut Store) -> Result<u64, Error> {
    ensure_triggers(store)?;
    let mine = store.qualified("stack_axis");
    store.execute(&format!("DELETE FROM {mine}"), &[])?;
    let rows = fresh(store, None);
    let made = store.execute(&format!("INSERT INTO {mine} ({}) {rows}", columns()), &[])?;
    // Postgres plans a question from what it last counted of a table, and
    // of rows just written it has counted nothing: until its own sweep
    // came by, a minute or more, the first questions after a rebuild
    // would be planned for an empty table.
    if let Store::Postgres { .. } = store {
        store.batch(&format!("ANALYZE {mine}"))?;
    }
    Ok(made)
}

/// How the table stands against its sources.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Standing {
    /// Stacks with a row.
    pub stacks: u64,
    /// Stacks one of the sources has and no row here.
    pub missing: u64,
    /// Rows here of stacks none of the sources has.
    pub extra: u64,
    /// Rows that say something else than their sources do.
    pub different: u64,
    /// Some of the stacks that differ, lowest first.
    pub sample: Vec<i64>,
    /// Whether the triggers that keep the stack's place and the
    /// fingerprint columns are set ([`triggers`]).
    pub triggers: bool,
    /// Which of them are not, and how: missing, switched off, or running
    /// a function that does not write this registry's rows.
    #[serde(default)]
    pub lost: Triggers,
}

impl Standing {
    /// Every stack's row says what its sources say, and the triggers that
    /// keep it so are set.
    pub fn agrees(&self) -> bool {
        self.missing == 0 && self.extra == 0 && self.different == 0 && self.triggers
    }
}

/// Compare every column of every row against its sources, and look for
/// the triggers.
pub fn verify(store: &mut Store) -> Result<Standing, Error> {
    let mine = store.qualified("stack_axis");
    let rows = fresh(store, None);
    let lost = triggers(store)?;
    let mut out = Standing {
        stacks: store.query(&format!("SELECT COUNT(*) FROM {mine}"), &[])?[0].int(0)? as u64,
        triggers: lost.set(),
        lost,
        ..Standing::default()
    };
    // a null on one side only is a difference, which `<>` would not say
    let differs = match store {
        Store::Sqlite(_) => "IS NOT",
        Store::Postgres { .. } => "IS DISTINCT FROM",
    };
    let other: String = ["series_id", "fingerprinted"]
        .iter()
        .chain(HOT)
        .map(|c| format!(" OR x.{c} {differs} n.{c}"))
        .collect();
    let differing = format!(
        "SELECT COALESCE(n.stack_id, x.stack_id) AS stack_id, \
         CASE WHEN x.stack_id IS NULL THEN 0 WHEN n.stack_id IS NULL THEN 1 ELSE 2 END AS how \
         FROM ({rows}) n FULL JOIN {mine} x ON x.stack_id = n.stack_id \
         WHERE x.stack_id IS NULL OR n.stack_id IS NULL OR x.axes <> n.axes \
         OR x.excluded <> n.excluded{other}"
    );
    for r in store.query(
        &format!("SELECT d.how, COUNT(*) FROM ({differing}) d GROUP BY d.how"),
        &[],
    )? {
        let n = r.int(1)? as u64;
        match r.int(0)? {
            0 => out.missing = n,
            1 => out.extra = n,
            _ => out.different = n,
        }
    }
    if !out.agrees() {
        out.sample = store
            .query(
                &format!("SELECT d.stack_id FROM ({differing}) d ORDER BY d.stack_id LIMIT 20"),
                &[],
            )?
            .iter()
            .map(|r| r.int(0))
            .collect::<Result<_, _>>()?;
    }
    Ok(out)
}
