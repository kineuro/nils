// SPDX-License-Identifier: AGPL-3.0-only

//! A stack set reads a stack's fingerprint columns from its row of the read
//! model (`nils_registry::stack_axis`), and reads there exactly what it
//! read through the fingerprint's own row.
//!
//! The synthetic registry is given the stacks where the two could part: a
//! stack with no fingerprint (which the join left out of every set), one
//! with every spacing unknown, with one spacing unknown, with the
//! acquisition type filled, read, both or neither. Each is written to
//! `stack_fingerprint` as SQL by hand, with no call to the read model, so
//! the rows a question then reads are the ones the database kept. Every
//! question here is then answered twice, from the row and through the join
//! (`compile::fingerprint_by_join`, which is the statement as it was), on
//! both backends: the same rows, value for value. The derived fields are
//! the ones a null takes part in differently on each backend (`COALESCE`
//! of the filled and the read type, the largest and the smallest of
//! spacings some of which are unknown), in a clause, under `not`, under
//! `is_null`, as a column and as a group's key.

mod common;

use common::{Lab, ask_of};
use nils_ask::compile::fingerprint_by_join;
use nils_registry::stack_axis;
use nils_registry::store::{Cell, Row};

fn labs() -> Vec<Lab> {
    common::labs("nils_stack_row_test")
}

fn run(l: &mut Lab, sql: &str) {
    let store = l.registry.store();
    let sql = sql.replace("{print}", &store.qualified("stack_fingerprint"));
    store.execute(&sql, &[]).unwrap();
}

/// What a question answers from the row, having answered the same through
/// the join.
fn both(l: &mut Lab, what: &str, text: &str) -> Vec<Row> {
    let (from_row, a) = ask_of(l, text);
    fingerprint_by_join(true);
    let (through_join, b) = ask_of(l, text);
    fingerprint_by_join(false);
    assert_eq!(a.rows, b.rows, "{}: {what}", l.name);
    // the two are the two statements: one ties the fingerprint, one does not
    assert!(
        through_join.sql.contains("stack_fingerprint f"),
        "{}: {what}: {}",
        l.name,
        through_join.sql
    );
    assert!(
        !from_row.sql.contains("stack_fingerprint"),
        "{}: {what}: {}",
        l.name,
        from_row.sql
    );
    a.rows
}

fn count(l: &mut Lab, what: &str, clauses: &str) -> i64 {
    let text = format!(
        "ast_version: 1\nname: {what}\nsets:\n  scans:\n    grain: stack{}\nout: {{set: scans, level: count}}\n",
        if clauses.is_empty() {
            String::new()
        } else {
            format!("\n    where:\n{clauses}")
        }
    );
    both(l, what, &text)[0].int(0).unwrap()
}

const LISTED: &str = r#"
ast_version: 1
name: every stack with what is derived of its fingerprint
sets:
  scans: {grain: stack}
out:
  set: scans
  level: record
  columns:
    - ["field", {}, "id"]
    - ["derived", {}, "acquisition_type"]
    - ["derived", {}, "voxel"]
    - ["derived", {}, "voxel_min"]
    - ["derived", {}, "voxel_max"]
    - ["field", {}, "slice_thickness"]
    - ["field", {}, "pixel_spacing_row"]
    - ["field", {}, "pixel_spacing_col"]
  order:
    - [["field", {}, "id"], asc]
"#;

const GROUPED: &str = r#"
ast_version: 1
name: stacks by their acquisition type and by their largest spacing
sets:
  scans:
    grain: stack
    bind:
      kind: ["derived", {}, "acquisition_type"]
      coarsest: ["derived", {}, "voxel_max"]
  by_type:
    grain: group
    group:
      of: scans
      by:
        - ["field", {}, "kind"]
        - ["field", {}, "coarsest"]
    bind:
      thinnest: ["min", {set: scans}, ["field", {}, "slice_thickness"]]
out:
  set: by_type
  level: record
  columns:
    - ["field", {}, "kind"]
    - ["field", {}, "coarsest"]
    - ["field", {}, "_rows"]
    - ["field", {}, "thinnest"]
"#;

/// The row of one stack in [`LISTED`]'s answer.
fn listed(rows: &[Row], stack: i64) -> Option<&Row> {
    rows.iter()
        .find(|r| r.0.contains(&Cell::Int(stack)) && r.0[2] == Cell::Int(stack))
}

#[test]
fn a_stack_set_reads_from_the_row_what_it_read_through_the_join() {
    for mut l in labs() {
        let name = l.name;
        // eight stacks of the registry, to be the awkward ones
        let ids: Vec<i64> = {
            let store = l.registry.store();
            let sql = format!(
                "SELECT stack_id FROM {} ORDER BY stack_id LIMIT 8",
                store.qualified("stack_fingerprint")
            );
            store
                .query(&sql, &[])
                .unwrap()
                .iter()
                .map(|r| r.int(0).unwrap())
                .collect()
        };
        assert_eq!(ids.len(), 8, "{name}");
        let all = count(&mut l, "every stack", "");
        let before = both(&mut l, "listed", LISTED);
        assert_eq!(before.len() as i64, all, "{name}");
        assert!(listed(&before, ids[0]).is_some(), "{name}");

        // no fingerprint at all
        run(
            &mut l,
            &format!("DELETE FROM {{print}} WHERE stack_id = {}", ids[0]),
        );
        // every spacing unknown, one unknown, another unknown
        run(
            &mut l,
            &format!(
                "UPDATE {{print}} SET pixel_spacing_row = NULL, pixel_spacing_col = NULL, \
                 slice_thickness = NULL WHERE stack_id = {}",
                ids[1]
            ),
        );
        run(
            &mut l,
            &format!(
                "UPDATE {{print}} SET pixel_spacing_row = NULL, pixel_spacing_col = 0.4, \
                 slice_thickness = 6.0 WHERE stack_id = {}",
                ids[2]
            ),
        );
        run(
            &mut l,
            &format!(
                "UPDATE {{print}} SET pixel_spacing_row = 0.9, pixel_spacing_col = 0.8, \
                 slice_thickness = NULL WHERE stack_id = {}",
                ids[3]
            ),
        );
        // the acquisition type read and not filled, both, neither, filled
        // and not read
        for (stack, filled, read) in [
            (ids[4], "NULL", "'3D'"),
            (ids[5], "'2D'", "'3D'"),
            (ids[6], "NULL", "NULL"),
            (ids[7], "'3D'", "NULL"),
        ] {
            run(
                &mut l,
                &format!(
                    "UPDATE {{print}} SET acquisition_type_filled = {filled}, \
                     mr_acquisition_type = {read} WHERE stack_id = {stack}"
                ),
            );
        }
        // nothing told the read model: the database kept it
        let standing = stack_axis::verify(l.registry.store()).unwrap();
        assert!(standing.agrees(), "{name}: {standing:?}");

        // a stack with no fingerprint is in no set, as the join left it out
        assert_eq!(count(&mut l, "every stack", ""), all - 1, "{name}");
        let after = both(&mut l, "listed", LISTED);
        assert!(listed(&after, ids[0]).is_none(), "{name}");
        // the derived fields of the awkward stacks, as a column each
        let of = |stack: i64| listed(&after, stack).unwrap_or_else(|| panic!("{name}: {stack}"));
        // columns: 2 the id, 3 the type, 4 voxel, 5 voxel_min, 6 voxel_max
        assert_eq!(
            of(ids[1]).0[4..7],
            [Cell::Null, Cell::Null, Cell::Null],
            "{name}"
        );
        // One spacing unknown: the smallest and the largest of the others
        // on Postgres, whose `LEAST` and `GREATEST` pass an unknown over,
        // and unknown on SQLite, whose `MIN` and `MAX` of several values
        // are unknown where one is. Each backend as it always answered:
        // the row changes neither.
        let one_unknown = |known: [f64; 2]| {
            if name == "postgres" {
                [Cell::Double(known[0]), Cell::Double(known[1])]
            } else {
                [Cell::Null, Cell::Null]
            }
        };
        assert_eq!(of(ids[2]).0[5..7], one_unknown([0.4, 6.0]), "{name}");
        assert_eq!(of(ids[3]).0[5..7], one_unknown([0.8, 0.9]), "{name}");
        assert_eq!(of(ids[4]).0[3], Cell::Text("3D".into()), "{name}");
        assert_eq!(of(ids[5]).0[3], Cell::Text("2D".into()), "{name}");
        assert_eq!(of(ids[6]).0[3], Cell::Null, "{name}");
        assert_eq!(of(ids[7]).0[3], Cell::Text("3D".into()), "{name}");
        both(&mut l, "grouped", GROUPED);

        // in a clause, under `not`, under `is_null`, by an order
        let mut met = 0;
        for (what, clauses) in [
            (
                "3D",
                r#"      - ["=", {}, ["derived", {}, "acquisition_type"], "3D"]"#,
            ),
            (
                "not 3D",
                r#"      - ["not", {}, ["=", {}, ["derived", {}, "acquisition_type"], "3D"]]"#,
            ),
            (
                "other than 3D",
                r#"      - ["<>", {}, ["derived", {}, "acquisition_type"], "3D"]"#,
            ),
            (
                "2D or 3D",
                r#"      - ["in", {}, ["derived", {}, "acquisition_type"], ["2D", "3D"]]"#,
            ),
            (
                "no type",
                r#"      - ["is_null", {}, ["derived", {}, "acquisition_type"]]"#,
            ),
            (
                "fine",
                r#"      - ["<=", {}, ["derived", {}, "voxel"], 1.0]"#,
            ),
            (
                "not fine",
                r#"      - ["not", {}, ["<=", {}, ["derived", {}, "voxel"], 1.0]]"#,
            ),
            (
                "coarse",
                r#"      - [">", {}, ["derived", {}, "voxel"], 1.0]"#,
            ),
            (
                "no voxel",
                r#"      - ["is_null", {}, ["derived", {}, "voxel"]]"#,
            ),
            (
                "thin at least",
                r#"      - [">=", {}, ["derived", {}, "voxel_min"], 0.5]"#,
            ),
            (
                "not thin",
                r#"      - ["not", {}, ["<", {}, ["derived", {}, "voxel_min"], 0.5]]"#,
            ),
            (
                "thick at most",
                r#"      - ["<=", {}, ["derived", {}, "voxel_max"], 5.0]"#,
            ),
            (
                "not thick",
                r#"      - ["not", {}, [">", {}, ["derived", {}, "voxel_max"], 5.0]]"#,
            ),
            (
                "a number against the field",
                r#"      - [">=", {}, 5.0, ["derived", {}, "voxel_max"]]"#,
            ),
            (
                "no thickness",
                r#"      - ["is_null", {}, ["field", {}, "slice_thickness"]]"#,
            ),
            (
                "a spacing by itself",
                r#"      - ["<", {}, ["field", {}, "pixel_spacing_row"], 0.95]"#,
            ),
            (
                "several together",
                "      - [\"=\", {}, [\"derived\", {}, \"acquisition_type\"], \"3D\"]\n      - [\"<=\", {}, [\"derived\", {}, \"voxel\"], 1.2]\n      - [\"=\", {}, [\"axis\", {}, \"base\"], \"T1w\"]",
            ),
        ] {
            let n = count(&mut l, what, clauses);
            met += i64::from(n > 0);
        }
        // the questions are ones the registry has stacks for
        assert!(met >= 12, "{name}: {met}");
        // the stacks with no type and with no voxel are the ones made so
        assert!(
            count(
                &mut l,
                "no type",
                r#"      - ["is_null", {}, ["derived", {}, "acquisition_type"]]"#
            ) >= 1,
            "{name}"
        );
        assert!(
            count(
                &mut l,
                "no voxel",
                r#"      - ["is_null", {}, ["derived", {}, "voxel"]]"#
            ) >= 1,
            "{name}"
        );
    }
}
