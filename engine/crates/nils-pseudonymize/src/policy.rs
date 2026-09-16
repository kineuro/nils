// SPDX-License-Identifier: AGPL-3.0-only

//! The pseudonymiser's tag policy, as numbers (decision record 28).
//!
//! What it does to the standard elements it acts on: the four categories it
//! removes tag for tag, the element the subject's code goes into, the two
//! that are never removed, and the covariates a dataset keeps unless it opts
//! out. The times are a release's and not the pseudonymiser's, so they are
//! not here.
//!
//! The engine holds no tag names and this states none. A number, its
//! category and what becomes of it are the policy, which is the engine's;
//! the words a reader needs are not.
//!
//! A dataset's own `keep` and `remove` lists are that dataset's, and are
//! served with the dataset rather than here.

use dicom_core::Tag;
use dicom_dictionary_std::tags;
use nils_release::tags::{Category, MANDATORY};
use serde_json::{Value, json};

use crate::rewrite::CATEGORIES;
use crate::settings::DEMOGRAPHICS;

/// What can become of an element here, three of the actions a release
/// records, in the words `nils_release::scrub` counts them under. One word
/// per behaviour across the system, so that an audit and a chooser never
/// describe one act differently; the detail belongs in the `why`.
pub const FATES: [&str; 3] = ["removed", "replaced", "kept"];

/// A tag as a dataset's own lists are written, `gggg,eeee`.
fn text(tag: Tag) -> String {
    format!("{:04X},{:04X}", tag.group(), tag.element())
}

/// What becomes of one element of the categories, and why where it is not
/// the plain removal.
fn fate(tag: Tag) -> (&'static str, Option<&'static str>) {
    if tag == tags::PATIENT_AGE {
        return (
            "replaced",
            Some(
                "computed from the birth date and the study date and put in place of whatever was there, before the birth date goes, where the file carries both and no age of its own",
            ),
        );
    }
    if DEMOGRAPHICS.contains(&tag) {
        return (
            "kept",
            Some("a covariate, kept unless the dataset opts out"),
        );
    }
    ("removed", None)
}

/// Why an element is never removed, whatever a category says.
fn never(tag: Tag) -> &'static str {
    if tag == tags::SOP_CLASS_UID {
        "what says how to read the file; without it no reader opens it"
    } else {
        "what names this one instance; the copy keeps it, so a tree holding it already holds this file"
    }
}

/// The policy, for the door and for anything else that has to state it.
pub fn document() -> Value {
    let mut listed: Vec<(Tag, Category)> = CATEGORIES
        .iter()
        .flat_map(|c| c.tags().iter().map(move |(g, e)| (Tag(*g, *e), *c)))
        .collect();
    listed.sort_unstable_by_key(|(tag, _)| *tag);
    let tags_of: Vec<Value> = listed
        .iter()
        .map(|(tag, category)| {
            let (fate, why) = fate(*tag);
            let mut row = json!({"tag": text(*tag), "category": category.name(), "fate": fate});
            if let Some(why) = why {
                row["why"] = Value::from(why);
            }
            row
        })
        .collect();
    json!({
        "count": tags_of.len(),
        "categories": CATEGORIES
            .iter()
            .map(|c| json!({"category": c.name(), "count": c.tags().len()}))
            .collect::<Vec<_>>(),
        "fates": FATES,
        "tags": tags_of,
        "code": {
            "tag": text(tags::PATIENT_ID),
            "fate": "replaced",
            "why": "the subject's code, which the linkage store and the registry's key decide; a run never chooses one of its own",
        },
        "mandatory": MANDATORY
            .iter()
            .map(|(g, e)| {
                let tag = Tag(*g, *e);
                json!({"tag": text(tag), "fate": "kept", "why": never(tag)})
            })
            .collect::<Vec<_>>(),
        "covariates": {
            "fate": "kept",
            "opt_out": "keep_demographics",
            "tags": DEMOGRAPHICS.iter().map(|t| text(*t)).collect::<Vec<_>>(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rewrite::Scrub;
    use crate::settings::{TagLists, parse_tag};
    use dicom_core::{DataElement, PrimitiveValue, VR};
    use dicom_object::{DefaultDicomObject, FileMetaTableBuilder, InMemDicomObject};
    use nils_release::scrub;

    fn list(doc: &Value, key: &str) -> Vec<Value> {
        doc[key].as_array().expect("an array").clone()
    }

    fn fate_of(doc: &Value, tag: &str) -> String {
        list(doc, "tags")
            .into_iter()
            .find(|r| r["tag"] == tag)
            .unwrap_or_else(|| panic!("{tag} is not served"))["fate"]
            .as_str()
            .expect("a fate")
            .to_string()
    }

    #[test]
    fn the_hundred_are_v0_s_four_categories_and_never_the_times() {
        let doc = document();
        assert_eq!(doc["count"], 100);
        assert_eq!(
            doc["categories"],
            json!([
                {"category": "patient", "count": 34},
                {"category": "trial", "count": 23},
                {"category": "provider", "count": 38},
                {"category": "institution", "count": 5},
            ])
        );
        let tags = list(&doc, "tags");
        assert_eq!(tags.len(), 100);
        // The times belong to a release and not to the pseudonymiser: the
        // fifth category is here neither by name nor by number.
        assert!(!doc.to_string().contains("times"), "{doc}");
        let served: Vec<&str> = tags.iter().map(|r| r["tag"].as_str().unwrap()).collect();
        for (g, e) in Category::Times.tags() {
            assert!(
                !served.contains(&text(Tag(*g, *e)).as_str()),
                "{g:04X},{e:04X}"
            );
        }
        // Every tag reads back the way a dataset's own lists are written, so
        // a reader can match the two, and none is served twice.
        let mut parsed: Vec<Tag> = served.iter().map(|t| parse_tag(t).unwrap()).collect();
        let n = parsed.len();
        parsed.sort_unstable();
        parsed.dedup();
        assert_eq!(parsed.len(), n);
        let mut sorted = served.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, served, "served in tag order");
    }

    #[test]
    fn the_two_that_make_a_file_a_file_are_named_and_never_listed_for_removal() {
        let doc = document();
        let mandatory = list(&doc, "mandatory");
        assert_eq!(mandatory.len(), 2);
        assert_eq!(mandatory[0]["tag"], "0008,0016");
        assert_eq!(mandatory[1]["tag"], "0008,0018");
        let served: Vec<String> = list(&doc, "tags")
            .iter()
            .map(|r| r["tag"].as_str().unwrap().to_string())
            .collect();
        for row in &mandatory {
            assert_eq!(row["fate"], "kept", "{row}");
            assert!(!row["why"].as_str().unwrap().is_empty(), "{row}");
            assert!(
                !served.contains(&row["tag"].as_str().unwrap().to_string()),
                "{row}"
            );
        }
    }

    #[test]
    fn the_fates_are_the_three_the_pseudonymiser_reaches() {
        let doc = document();
        assert_eq!(doc["fates"], json!(["removed", "replaced", "kept"]));
        // The age is replaced rather than removed, though its category holds
        // it: it is computed and put in place of what the file carried. The
        // three covariates are kept.
        assert_eq!(fate_of(&doc, "0010,1010"), "replaced");
        for covariate in ["0010,0040", "0010,1020", "0010,1030"] {
            assert_eq!(fate_of(&doc, covariate), "kept", "{covariate}");
        }
        assert_eq!(doc["covariates"]["opt_out"], "keep_demographics");
        assert_eq!(
            doc["covariates"]["tags"],
            json!(["0010,0040", "0010,1030", "0010,1020"])
        );
        let removed = list(&doc, "tags")
            .iter()
            .filter(|r| r["fate"] == "removed")
            .count();
        assert_eq!(removed, 96);
        // The code's element is replaced and is in no category, which is why
        // a person is never offered it to keep.
        assert_eq!(doc["code"]["tag"], "0010,0020");
        assert_eq!(doc["code"]["fate"], "replaced");
        assert!(
            !list(&doc, "tags").iter().any(|r| r["tag"] == "0010,0020"),
            "{doc}"
        );
    }

    /// One file carrying every element the door names, rewritten under the
    /// plan the pseudonymiser builds, so that what is served is what a file
    /// meets and not a second copy of it.
    fn file(doc: &Value) -> DefaultDicomObject {
        let mut ds = InMemDicomObject::new_empty();
        let mut put = |tag: Tag, vr: VR, value: &str| {
            ds.put(DataElement::new(tag, vr, PrimitiveValue::from(value)));
        };
        for row in doc["tags"].as_array().unwrap() {
            let tag = parse_tag(row["tag"].as_str().unwrap()).unwrap();
            if tag == tags::PATIENT_AGE {
                // absent, so that the run writes one
                continue;
            }
            if tag == tags::PATIENT_BIRTH_DATE {
                put(tag, VR::DA, "19800615");
            } else {
                put(tag, VR::LO, "something identifying");
            }
        }
        put(tags::PATIENT_ID, VR::LO, "19800615-1234");
        put(tags::STUDY_DATE, VR::DA, "20220115");
        put(tags::SOP_CLASS_UID, VR::UI, "1.2.840.10008.5.1.4.1.1.4");
        put(tags::SOP_INSTANCE_UID, VR::UI, "1.2.3.4.5");
        ds.with_meta(
            FileMetaTableBuilder::new()
                .transfer_syntax("1.2.840.10008.1.2.1")
                .media_storage_sop_class_uid("1.2.840.10008.5.1.4.1.1.4")
                .media_storage_sop_instance_uid("1.2.3.4.5"),
        )
        .expect("a meta table")
    }

    #[test]
    fn a_file_rewritten_meets_every_fate_the_door_serves() {
        let doc = document();
        // the lists of a dataset that declares none: the covariates kept
        let lists = TagLists::of(&Value::Null).unwrap();
        let scrub = Scrub::new(&[], &lists.keep, &lists.remove);
        let mut object = file(&doc);
        let applied = scrub::apply(&mut object, &scrub.plan("abc123def456"));
        let value = |o: &DefaultDicomObject, tag: Tag| -> Option<String> {
            let e = o.element_opt(tag).ok().flatten()?;
            e.value().to_str().ok().map(|v| v.trim().to_string())
        };
        for row in doc["tags"].as_array().unwrap() {
            let tag = parse_tag(row["tag"].as_str().unwrap()).unwrap();
            let there = object.element_opt(tag).ok().flatten().is_some();
            match row["fate"].as_str().unwrap() {
                "removed" => assert!(!there, "{row} is still in the file"),
                "kept" | "replaced" => assert!(there, "{row} left the file"),
                other => panic!("{other} is not a fate of the pseudonymiser: {row}"),
            }
        }
        assert_eq!(applied.total("removed"), 96);
        assert_eq!(value(&object, tags::PATIENT_AGE).as_deref(), Some("041Y"));
        assert_eq!(
            value(&object, tags::PATIENT_ID).as_deref(),
            Some("abc123def456"),
            "the code, not the identifier it came in with"
        );
        for row in doc["mandatory"].as_array().unwrap() {
            let tag = parse_tag(row["tag"].as_str().unwrap()).unwrap();
            assert!(value(&object, tag).is_some(), "{row} left the file");
        }
        // The study date is the science and no category holds it.
        assert_eq!(
            value(&object, tags::STUDY_DATE).as_deref(),
            Some("20220115")
        );
    }
}
