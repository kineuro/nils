// SPDX-License-Identifier: AGPL-3.0-only

//! Record 55 K7, small lists at plain (the ruling of 2026-10-13), end to
//! end on each backend. It completes small counts at plain (2026-10-12): a
//! list's length is a count, so at detail plain a list of the rows a quasi
//! identifying field chose that holds 1 to 4 subjects answers no row, says
//! why in `held_back`, and gives no row count; none and 5 or more are
//! answered as before. A handle frozen from such an answer holds no row a
//! later page could read, and no door reads its length. Lists no quasi
//! identifying field chose, and every list at detail quasi, are unchanged.
//!
//! The synthetic registry's subjects with an age at their first visit: 2
//! of 45 or older, 36 of 35 or older.

mod common;

use common::Lab;
use nils_ask::affordance::{self, Setting};
use nils_ask::diagnose;
use nils_ask::exec::{Bounds, HeldBack};
use nils_ask::handle::{self, HandleError};
use nils_ask::parse;
use nils_ask::promote::{PromoteError, promote};
use nils_ask::run::{Outcome, Request, RunError, cached, run};
use nils_ask::validate::{Scope, held_list_why};
use nils_registry::session::Scheme;

fn labs() -> Vec<Lab> {
    common::labs("nils_small_lists_test")
}

fn bounds() -> Bounds {
    Bounds {
        timeout_ms: 20_000,
        max_rows: 5_000,
        max_bytes: 4 * 1024 * 1024,
    }
}

const SETS: &str = r#"
  people: {grain: subject}
  visits:
    grain: session
    of: people
    bind:
      n:   ["ordinal", {}]
      age: ["age_at", {}, ["field", {}, "subject.birth_date"], ["field", {}, "first"]]
  firsts:
    grain: session
    from: visits
    where:
      - ["=", {}, ["field", {}, "n"], 1]
      - ["not_null", {}, ["field", {}, "age"]]
"#;

/// The first visits at an age of `from` or more, one row per subject, at
/// `level`.
fn aged_from(from: i64, level: &str) -> String {
    format!(
        r#"ast_version: 1
name: small lists
scheme: default
sets:{SETS}  older:
    grain: session
    from: firsts
    where: [[">=", {{}}, ["field", {{}}, "age"], {from}]]
out:
  set: older
  level: {level}
  columns: [["field", {{}}, "n"]]
"#
    )
}

struct Asked<'a> {
    after: Option<i64>,
    limit: Option<u64>,
    keep: bool,
    scope: &'a Scope,
}

fn go(l: &mut Lab, text: &str, how: Asked<'_>) -> Result<Outcome, RunError> {
    let scheme = Scheme::default();
    run(
        &mut l.registry,
        Request {
            ask: parse(text).unwrap_or_else(|e| panic!("{}: {e}", l.name)),
            names: &l.catalog,
            scope: how.scope,
            principal: "tester",
            node: "lab",
            pack_version: Some("0.0.0-test"),
            scheme: &scheme,
            bounds: bounds(),
            page_rows: 50,
            name: None,
            keep: how.keep,
            after: how.after,
            limit: how.limit,
            may_project_raw: false,
            purpose: None,
            reader: None,
        },
    )
}

fn ok(l: &mut Lab, text: &str, scope: &Scope) -> Outcome {
    go(
        l,
        text,
        Asked {
            after: None,
            limit: None,
            keep: false,
            scope,
        },
    )
    .unwrap_or_else(|e| panic!("{}: {e}", l.name))
}

fn plain() -> Scope {
    Scope::default()
}

fn all_held(columns: &[String]) -> Vec<HeldBack> {
    columns
        .iter()
        .map(|c| HeldBack {
            column: c.clone(),
            why: held_list_why(),
        })
        .collect()
}

#[test]
fn a_list_an_age_chose_of_fewer_than_five_is_held_whole_at_plain() {
    for mut l in labs() {
        assert_eq!(
            held_list_why(),
            "a list of fewer than 5 subjects that a quasi identifying field chose; its length is a count, so no row is shown"
        );
        // at detail quasi: the 2 subjects' first visits
        let raw = ok(&mut l, &aged_from(45, "record"), &common::quasi());
        assert_eq!(raw.answer.rows.len(), 2, "{}", l.name);
        assert!(!raw.answer.rows_held);
        assert_eq!(raw.answer.row_count(), Some(2));

        // at detail plain: no row, every column held back with why, and no
        // length; the answer is not cut, so nothing says it was long
        let held = ok(&mut l, &aged_from(45, "record"), &plain());
        assert!(held.answer.rows.is_empty(), "{}", l.name);
        assert!(held.answer.rows_held, "{}", l.name);
        assert!(!held.answer.truncated);
        assert_eq!(held.answer.row_count(), None);
        assert_eq!(held.answer.columns, vec!["_key", "_subject", "n"]);
        assert_eq!(
            held.answer.held_back_named(),
            all_held(&held.answer.columns),
            "{}",
            l.name
        );

        // the handle holds no row, no page reads one, and it says why
        let h = handle::get(l.registry.store(), held.handle.id)
            .unwrap()
            .unwrap();
        assert!(h.rows_held(), "{}", l.name);
        assert_eq!(h.row_count_shown(), None);
        assert_eq!(h.held_back(), all_held(&held.answer.columns));
        assert!(handle::keys(l.registry.store(), h.id).unwrap().is_empty());
        assert_eq!(handle::page_count(l.registry.store(), h.id).unwrap(), 0);
        assert!(
            handle::page(l.registry.store(), h.id, 0)
                .unwrap()
                .unwrap_or_default()
                .is_empty(),
            "{}",
            l.name
        );

        // the cache answers the same handle, held as it was
        let scheme = Scheme::default();
        let again = cached(
            &mut l.registry,
            parse(&aged_from(45, "record")).unwrap(),
            &l.catalog,
            &plain(),
            Some("0.0.0-test"),
            &scheme,
        )
        .unwrap()
        .map(|(h, _)| h);
        if let Some(h) = again {
            assert!(h.rows_held(), "{}", l.name);
        }

        // an aggregate level lists one row per member too
        let aggregate = ok(&mut l, &aged_from(45, "aggregate"), &plain());
        assert!(aggregate.answer.rows_held && aggregate.answer.rows.is_empty());

        // a page of it, or a limit of one row, is held as the whole is:
        // the set's subjects decide, never the rows a page shows
        for (after, limit) in [(None, Some(1)), (Some(0), Some(1)), (Some(10_000), None)] {
            let paged = go(
                &mut l,
                &aged_from(45, "record"),
                Asked {
                    after,
                    limit,
                    keep: false,
                    scope: &plain(),
                },
            )
            .unwrap();
            assert!(
                paged.answer.rows_held && paged.answer.rows.is_empty(),
                "{}: {after:?} {limit:?}",
                l.name
            );
        }
    }
}

#[test]
fn none_and_five_or_more_are_listed_at_plain() {
    for mut l in labs() {
        // 36 subjects of 35 or older: listed, one row each
        let many = ok(&mut l, &aged_from(35, "record"), &plain());
        assert_eq!(many.answer.rows.len(), 36, "{}", l.name);
        assert!(!many.answer.rows_held);
        assert!(many.answer.held_back_named().is_empty());
        assert_eq!(many.answer.row_count(), Some(36));
        let h = handle::get(l.registry.store(), many.handle.id)
            .unwrap()
            .unwrap();
        assert!(!h.rows_held());
        assert_eq!(h.row_count_shown(), Some(36));
        // none: an empty list, as before, and not held
        let none = ok(&mut l, &aged_from(200, "record"), &plain());
        assert!(none.answer.rows.is_empty() && !none.answer.rows_held);
        assert_eq!(none.answer.row_count(), Some(0));
        assert!(none.answer.held_back_named().is_empty());
    }
}

#[test]
fn lists_no_quasi_field_chose_and_lists_at_quasi_are_unchanged() {
    for mut l in labs() {
        // the visits numbered 6: 3 subjects, chosen by the visit number,
        // which is no quasi identifying field
        let sixth = format!(
            r#"ast_version: 1
name: sixth visits
scheme: default
sets:{SETS}  sixth:
    grain: session
    from: visits
    where: [["=", {{}}, ["field", {{}}, "n"], 6]]
out:
  set: sixth
  level: record
"#
        );
        let few = ok(&mut l, &sixth, &plain());
        assert_eq!(few.answer.rows.len(), 3, "{}", l.name);
        assert!(!few.answer.rows_held);
        // every list at detail quasi
        let raw = ok(&mut l, &aged_from(45, "record"), &common::quasi());
        assert_eq!(raw.answer.rows.len(), 2, "{}", l.name);
    }
}

#[test]
fn the_preview_a_group_and_a_kept_set_hold_the_list_too() {
    for mut l in labs() {
        let scope = plain();
        let scheme = Scheme::default();
        let setting = Setting {
            names: &l.catalog,
            scope: &scope,
            scheme: &scheme,
            principal: "tester",
            bounds: bounds(),
            values_cap: 50,
        };
        // the preview: no row, every column held with why
        let previewed = affordance::preview(
            &mut l.registry,
            &parse(&aged_from(45, "record")).unwrap(),
            10,
            &setting,
            None,
        )
        .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        assert!(previewed.rows.is_empty(), "{}", l.name);
        assert_eq!(previewed.held_back, all_held(&previewed.columns));
        let listed = affordance::preview(
            &mut l.registry,
            &parse(&aged_from(35, "record")).unwrap(),
            10,
            &setting,
            None,
        )
        .unwrap();
        assert_eq!(listed.rows.len(), 10);
        assert!(listed.held_back.is_empty());

        // a group of the few: its rows are the groups, and how many there
        // are says something of 2 subjects, so it is held whole
        let grouped = format!(
            r#"ast_version: 1
name: few by visit number
scheme: default
sets:{SETS}  older:
    grain: session
    from: firsts
    where: [[">=", {{}}, ["field", {{}}, "age"], 45]]
  by_n:
    grain: group
    group: {{of: older, by: [["field", {{}}, "n"]]}}
out:
  set: by_n
  level: record
  columns: [["field", {{}}, "n"]]
"#
        );
        let g = ok(&mut l, &grouped, &plain());
        assert!(g.answer.rows_held && g.answer.rows.is_empty(), "{}", l.name);
        let g_raw = ok(&mut l, &grouped, &common::quasi());
        assert_eq!(g_raw.answer.rows.len(), 1, "{}", l.name);

        // a kept set: the count of everyone, keeping the few; the kept
        // handle holds no row
        let kept_doc = format!(
            r#"ast_version: 1
name: everyone, keeping the few
scheme: default
sets:{SETS}  older:
    grain: session
    from: firsts
    where: [[">=", {{}}, ["field", {{}}, "age"], 45]]
keep: [older]
out:
  set: people
  level: count
"#
        );
        let kept = go(
            &mut l,
            &kept_doc,
            Asked {
                after: None,
                limit: None,
                keep: true,
                scope: &scope,
            },
        )
        .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        assert_eq!(kept.kept.len(), 1, "{}", l.name);
        assert!(kept.kept[0].rows_held(), "{}", l.name);
        assert_eq!(kept.kept[0].row_count_shown(), None);
        assert!(
            handle::keys(l.registry.store(), kept.kept[0].id)
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn a_held_handle_is_no_source_and_is_never_promoted() {
    for mut l in labs() {
        let held = ok(&mut l, &aged_from(45, "record"), &plain());
        let id = held.handle.id;
        common::refresh(&mut l);
        // read as a set's source it would answer none as if it were empty
        let from_it = format!(
            r#"ast_version: 1
name: from a held list
scheme: default
sets:
  t: {{grain: session, from: "handle:{id}"}}
out:
  set: t
  level: count
"#
        );
        match go(
            &mut l,
            &from_it,
            Asked {
                after: None,
                limit: None,
                keep: false,
                scope: &plain(),
            },
        ) {
            Err(RunError::Handle(HandleError::Held(h))) => assert_eq!(h, id),
            other => panic!("{}: {:?}", l.name, other.map(|o| o.answer.rows)),
        }
        // a cohort made of it would say none where there were some
        match promote(&mut l.registry, id, "the-few", "tester", None, true) {
            Err(PromoteError::Handle(HandleError::Held(h))) => assert_eq!(h, id),
            Err(e) => panic!("{}: {e}", l.name),
            Ok(_) => panic!("{}: a held handle was promoted", l.name),
        }
    }
}

#[test]
fn the_funnel_with_keys_holds_a_held_list_as_its_count() {
    for mut l in labs() {
        let scheme = Scheme::default();
        let d = diagnose::diagnose(
            &mut l.registry,
            parse(&aged_from(45, "record")).unwrap(),
            Vec::new(),
            &l.catalog,
            &plain(),
            &scheme,
            bounds(),
            true,
            None,
        )
        .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        let last = d.funnel.iter().rfind(|s| s.set == "older").unwrap();
        assert_eq!((last.rows, last.subjects), (None, None), "{}", l.name);
        assert!(last.keys.is_empty());
        assert!(last.held_back.is_some());
    }
}
