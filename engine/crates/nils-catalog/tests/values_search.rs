// SPDX-License-Identifier: AGPL-3.0-only

//! The value sampler finds a text that is not among the most common
//! values.
//!
//! The sampler lists a field's most common values under a cap, so a word
//! few rows hold (a niche product name in a series description) was never
//! among them. With `contains` it answers the values that hold a text,
//! letter case aside, with their counts, the most common first, under the
//! same cap. Only a text field whose values are listed is searched: a
//! field shown as shapes is not, since a search would say whether a value
//! is there that the listing does not show; and the text is three
//! characters or more.

mod common;

use common::quasi;
use nils_ask::affordance::{self, AffordanceError, CONTAINS_MIN, Sample, Setting};
use nils_ask::exec::Bounds;
use nils_ask::validate::Scope;
use nils_registry::session::Scheme;

fn bounds() -> Bounds {
    Bounds {
        timeout_ms: 20_000,
        max_rows: 5_000,
        max_bytes: 4 * 1024 * 1024,
    }
}

#[test]
fn the_values_that_hold_a_text_are_found_with_their_counts() {
    for mut l in common::labs("nils_values_search_test") {
        let scheme = Scheme::default();
        for scope in [quasi(), Scope::default()] {
            let s = Setting {
                names: &l.catalog,
                scope: &scope,
                scheme: &scheme,
                principal: "reader",
                bounds: bounds(),
                values_cap: 50,
            };
            let mut search = |field: &str, cap: usize, contains: Option<&str>| {
                affordance::values_containing(
                    &mut l.registry,
                    "stack",
                    field,
                    &s,
                    cap,
                    None,
                    contains,
                )
            };
            let field = "text_series_description";
            // every value the field holds, as the plain listing counts them
            let all: Sample = search(field, 4_000, None).unwrap();
            assert!(!all.truncated && all.items.len() > 3, "{}", l.name);
            assert_eq!(all.kind, "values", "{}", l.name);
            // a text from inside the least common value, in another case
            let (rare, _) = all
                .items
                .iter()
                .rfind(|(v, _)| v != "null" && v.chars().count() > CONTAINS_MIN)
                .unwrap();
            let needle: String = rare.chars().skip(1).take(CONTAINS_MIN + 1).collect();
            let needle = needle.to_uppercase();
            let mut expected: Vec<(String, i64)> = all
                .items
                .iter()
                .filter(|(v, _)| v.to_lowercase().contains(&needle.to_lowercase()))
                .cloned()
                .collect();
            assert!(!expected.is_empty(), "{}: {needle}", l.name);
            let found = search(field, 50, Some(&needle)).unwrap();
            assert_eq!(found.kind, "values", "{}", l.name);
            assert_eq!(
                found.distinct,
                expected.len() as i64,
                "{}: {needle}",
                l.name
            );
            assert!(!found.truncated, "{}", l.name);
            // the same values with the same counts, the most common first
            assert!(
                found.items.windows(2).all(|w| w[0].1 >= w[1].1),
                "{}: {:?}",
                l.name,
                found.items
            );
            let mut got = found.items.clone();
            got.sort();
            expected.sort();
            assert_eq!(got, expected, "{}: {needle}", l.name);
            // surrounding space is not part of the text
            let spaced = search(field, 50, Some(&format!("  {needle} "))).unwrap();
            assert_eq!(spaced.items, found.items, "{}", l.name);
            // under the cap: the most common of them, and said to be cut
            if expected.len() > 1 {
                let cut = search(field, 1, Some(&needle)).unwrap();
                assert_eq!(cut.items.len(), 1, "{}", l.name);
                assert_eq!(cut.items[0], found.items[0], "{}", l.name);
                assert!(cut.truncated, "{}", l.name);
                assert_eq!(cut.distinct, expected.len() as i64, "{}", l.name);
            }
            // a text no value holds
            let none = search(field, 50, Some("zzqqxx-no-such-text")).unwrap();
            assert!(none.items.is_empty() && none.distinct == 0, "{}", l.name);
            assert!(!none.truncated, "{}", l.name);
            // a text too short is refused, and told the least
            for short in ["", "ab", "  ab  "] {
                match search(field, 50, Some(short)) {
                    Err(AffordanceError::NoSearch { message, next }) => {
                        assert!(
                            message.starts_with("contains takes at least 3 characters"),
                            "{}: {message}",
                            l.name
                        );
                        assert!(next.contains("search for a longer text"), "{next}");
                    }
                    other => panic!("{}: {short:?} was searched: {:?}", l.name, other.is_ok()),
                }
            }
            // a field whose values are shown as shapes is not searched,
            // at any detail: its listing stays shapes
            match search("station_name", 50, Some("station")) {
                Err(AffordanceError::NoSearch { message, .. }) => {
                    assert!(message.contains("shown as shapes"), "{}: {message}", l.name);
                }
                other => panic!(
                    "{}: a shaped field was searched: {:?}",
                    l.name,
                    other.is_ok()
                ),
            }
            assert_eq!(
                search("station_name", 50, None).unwrap().kind,
                "shapes",
                "{}",
                l.name
            );
            // an axis is the catalog's own list
            match search("technique", 50, Some("mpr")) {
                Err(AffordanceError::NoSearch { message, .. }) => {
                    assert!(message.contains("is an axis"), "{}: {message}", l.name);
                }
                other => panic!("{}: an axis was searched: {:?}", l.name, other.is_ok()),
            }
            // and a name that is no field is refused as it always was
            assert!(
                matches!(
                    search("no_such_field", 50, Some("text")),
                    Err(AffordanceError::NoField { .. })
                ),
                "{}",
                l.name
            );
        }
    }
}
