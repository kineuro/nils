// SPDX-License-Identifier: AGPL-3.0-only

//! Applying a release's policy to one file
//! (`docs/specs/wave3-anonymize-and-bids.md`, §8).
//!
//! Everything here is a function of the plan and the dataset, so the same file
//! under the same plan gives the same output, and two releases of overlapping
//! selections agree byte for byte.
//!
//! The order matters and is the order below: read what is needed before
//! anything is removed (the age needs the birth date), then remove, then
//! replace, then remap. v0 removes first and so cannot compute an age at all,
//! which is why its output has neither the birth date nor the age that was
//! derivable from it.

use std::collections::BTreeMap;

use dicom_core::header::Header as _;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_dictionary_std::tags;
use dicom_object::{DefaultDicomObject, InMemDicomObject};

use crate::dates::{self, Offset};
use crate::policy::Policy;
use crate::tags::{Category, MANDATORY};
use crate::uid::Remap;
use nils_registry::day::Day;

/// What to do to one subject's files.
pub struct Plan<'a> {
    pub policy: &'a Policy,
    pub categories: &'a [Category],
    /// The pseudonym this subject's `PatientID` becomes. The registry chose
    /// it; the release does not choose a pseudonym of its own (§8.1).
    pub code: &'a str,
    pub offset: Offset,
    /// None when the policy preserves UIDs.
    pub remap: Option<&'a Remap>,
}

/// What was done, counted per tag so the audit can say so without saying what
/// the value was (§8.5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// Tag to action to count. The action is `removed`, `replaced`, `shifted`
    /// or `remapped`; there is deliberately no old value anywhere.
    pub changes: BTreeMap<(String, &'static str), i64>,
    /// The age it wrote, when it could compute one.
    pub age: Option<i64>,
}

impl Applied {
    fn note(&mut self, tag: Tag, action: &'static str) {
        *self
            .changes
            .entry((
                format!("({:04X},{:04X})", tag.group(), tag.element()),
                action,
            ))
            .or_insert(0) += 1;
    }

    pub fn total(&self, action: &str) -> i64 {
        self.changes
            .iter()
            .filter(|((_, a), _)| *a == action)
            .map(|(_, n)| n)
            .sum()
    }
}

/// Apply a plan to one dataset, in place.
pub fn apply(object: &mut DefaultDicomObject, plan: &Plan) -> Applied {
    let mut done = Applied::default();

    // 1. What has to be read before it is removed. The age is derivable from
    //    the archive and not from v0's output, because v0 removes the birth
    //    date without ever computing one.
    let born = text_of(object, tags::PATIENT_BIRTH_DATE).and_then(|v| Day::parse(&v));
    let studied = text_of(object, tags::STUDY_DATE).and_then(|v| Day::parse(&v));
    if let (Some(born), Some(studied)) = (born, studied)
        && object
            .element_opt(tags::PATIENT_AGE)
            .ok()
            .flatten()
            .is_none()
        && let Some(years) = dates::age_years(born, studied)
    {
        object.put(DataElement::new(
            tags::PATIENT_AGE,
            VR::AS,
            PrimitiveValue::from(dates::age_string(years)),
        ));
        done.note(tags::PATIENT_AGE, "replaced");
        done.age = Some(years);
    }

    // 2. The declared categories, less what makes a file a file.
    for tag in crate::tags::tags_of(plan.categories) {
        if MANDATORY.iter().any(|(g, e)| Tag(*g, *e) == tag) {
            continue;
        }
        // The age is written above and is not an identifier: v0's patient
        // category holds it, which is why v0 cannot both remove the birth date
        // and keep an age.
        if tag == tags::PATIENT_AGE || tag == tags::PATIENT_ID {
            continue;
        }
        if object.remove_element(tag) {
            done.note(tag, "removed");
        }
    }

    // 3. The identifier the registry chose.
    object.put(DataElement::new(
        tags::PATIENT_ID,
        VR::LO,
        PrimitiveValue::from(plan.code),
    ));
    done.note(tags::PATIENT_ID, "replaced");

    // 4. Every date, under the policy. Every one, rather than a list, because
    //    a list is what goes stale and because the intervals between them are
    //    what a reader measures on.
    if plan.policy.dates.moves_dates() {
        let dated: Vec<(Tag, VR, String)> = object
            .iter()
            .filter(|e| matches!(e.vr(), VR::DA | VR::DT))
            .filter_map(|e| {
                e.value()
                    .to_str()
                    .ok()
                    .map(|s| (e.tag(), e.vr(), s.to_string()))
            })
            .collect();
        for (tag, vr, raw) in dated {
            let Some(moved) = under(plan.policy.dates, plan.offset, vr, &raw) else {
                continue;
            };
            object.put(DataElement::new(tag, vr, PrimitiveValue::from(moved)));
            done.note(tag, "shifted");
        }
    }

    // 5. The UIDs, keyed and deterministic. Last, because everything above
    //    reads the dataset as it was.
    if let Some(remap) = plan.remap {
        let uids: Vec<(Tag, String)> = object
            .iter()
            .filter(|e| e.vr() == VR::UI)
            .filter_map(|e| {
                e.value()
                    .to_str()
                    .ok()
                    .map(|s| (e.tag(), s.trim().to_string()))
            })
            .filter(|(tag, v)| !v.is_empty() && !is_a_class(*tag))
            .collect();
        for (tag, old) in uids {
            object.put(DataElement::new(
                tag,
                VR::UI,
                PrimitiveValue::from(remap.of(&old)),
            ));
            done.note(tag, "remapped");
        }
        // The file meta carries the media storage instance UID, which is the
        // SOP instance UID again. A reader that trusted one and not the other
        // would see a file disagreeing with itself.
        let meta_uid = object.meta().media_storage_sop_instance_uid.clone();
        let new = remap.of(meta_uid.trim());
        object.meta_mut().media_storage_sop_instance_uid = new;
    }

    done
}

/// A transfer syntax or a SOP class is a UID that names a standard, not a
/// study. Remapping one would make the file unreadable.
fn is_a_class(tag: Tag) -> bool {
    matches!(
        tag,
        tags::SOP_CLASS_UID
            | tags::MEDIA_STORAGE_SOP_CLASS_UID
            | tags::TRANSFER_SYNTAX_UID
            | tags::IMPLEMENTATION_CLASS_UID
            | tags::SPECIFIC_CHARACTER_SET
    )
}

/// One date value under the policy, keeping whatever else a `DT` carries.
fn under(policy: dates::Policy, offset: Offset, vr: VR, raw: &str) -> Option<String> {
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }
    match vr {
        VR::DA => {
            let day = Day::parse(text)?;
            Some(dates::apply(policy, offset, day).compact())
        }
        VR::DT => {
            // `YYYYMMDDHHMMSS.FFFFFF&ZZXX`: the first eight are the date and
            // the rest is time, which the `times` category removes on its own
            // terms. Moving the date and keeping the rest is what keeps a
            // datetime a datetime.
            if text.len() < 8 {
                return None;
            }
            let day = Day::parse(&text[..8])?;
            Some(format!(
                "{}{}",
                dates::apply(policy, offset, day).compact(),
                &text[8..]
            ))
        }
        _ => None,
    }
}

fn text_of(object: &InMemDicomObject, tag: Tag) -> Option<String> {
    let e = object.element_opt(tag).ok().flatten()?;
    let v = e.value().to_str().ok()?;
    let v = v.trim();
    (!v.is_empty()).then(|| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Uids;
    use crate::uid::Root;
    use dicom_object::FileMetaTableBuilder;

    fn object(pairs: &[(Tag, VR, &str)]) -> DefaultDicomObject {
        let mut ds = InMemDicomObject::new_empty();
        for (tag, vr, value) in pairs {
            ds.put(DataElement::new(*tag, *vr, PrimitiveValue::from(*value)));
        }
        ds.with_meta(
            FileMetaTableBuilder::new()
                .transfer_syntax("1.2.840.10008.1.2.1")
                .media_storage_sop_class_uid("1.2.840.10008.5.1.4.1.1.4")
                .media_storage_sop_instance_uid("1.2.3.4.5"),
        )
        .expect("a meta table")
    }

    fn plan<'a>(policy: &'a Policy, remap: Option<&'a Remap>, offset: i64) -> Plan<'a> {
        Plan {
            policy,
            categories: &[
                Category::Patient,
                Category::Trial,
                Category::Provider,
                Category::Institution,
                Category::Times,
            ],
            code: "a1b2c3d4",
            offset: Offset(offset),
            remap,
        }
    }

    fn text(o: &DefaultDicomObject, tag: Tag) -> Option<String> {
        text_of(o, tag)
    }

    #[test]
    fn the_patient_is_the_code_the_registry_chose() {
        let mut o = object(&[
            (tags::PATIENT_ID, VR::LO, "19800101-1234"),
            (tags::PATIENT_NAME, VR::PN, "SVENSSON^ANNA"),
            (tags::STUDY_DATE, VR::DA, "20220115"),
        ]);
        let policy = Policy::default();
        apply(&mut o, &plan(&policy, None, 0));
        assert_eq!(text(&o, tags::PATIENT_ID).as_deref(), Some("a1b2c3d4"));
        assert_eq!(text(&o, tags::PATIENT_NAME), None, "the name is gone");
    }

    #[test]
    fn the_age_is_computed_before_the_birth_date_goes() {
        // v0 removes the birth date and computes nothing, so an age that was
        // derivable from the archive is not derivable from its output.
        let mut o = object(&[
            (tags::PATIENT_ID, VR::LO, "x"),
            (tags::PATIENT_BIRTH_DATE, VR::DA, "19800615"),
            (tags::STUDY_DATE, VR::DA, "20220115"),
        ]);
        let policy = Policy::default();
        let done = apply(&mut o, &plan(&policy, None, 0));
        assert_eq!(done.age, Some(41));
        assert_eq!(text(&o, tags::PATIENT_AGE).as_deref(), Some("041Y"));
        assert_eq!(text(&o, tags::PATIENT_BIRTH_DATE), None);
    }

    #[test]
    fn an_age_the_file_already_carries_is_left_alone() {
        let mut o = object(&[
            (tags::PATIENT_ID, VR::LO, "x"),
            (tags::PATIENT_AGE, VR::AS, "037Y"),
            (tags::PATIENT_BIRTH_DATE, VR::DA, "19800615"),
            (tags::STUDY_DATE, VR::DA, "20220115"),
        ]);
        let policy = Policy::default();
        apply(&mut o, &plan(&policy, None, 0));
        assert_eq!(text(&o, tags::PATIENT_AGE).as_deref(), Some("037Y"));
    }

    #[test]
    fn keeping_the_dates_writes_them_as_they_are() {
        let mut o = object(&[
            (tags::PATIENT_ID, VR::LO, "x"),
            (tags::STUDY_DATE, VR::DA, "20220115"),
        ]);
        let policy = Policy::default();
        let done = apply(&mut o, &plan(&policy, None, 37));
        assert_eq!(text(&o, tags::STUDY_DATE).as_deref(), Some("20220115"));
        assert_eq!(done.total("shifted"), 0);
    }

    #[test]
    fn a_shift_moves_every_date_in_the_file_by_the_same_amount() {
        // Every date rather than a list, because a list goes stale and because
        // the intervals between them are what a reader measures on.
        let mut o = object(&[
            (tags::PATIENT_ID, VR::LO, "x"),
            (tags::STUDY_DATE, VR::DA, "20220115"),
            (tags::SERIES_DATE, VR::DA, "20220115"),
            (tags::ACQUISITION_DATE, VR::DA, "20220116"),
            (tags::ACQUISITION_DATE_TIME, VR::DT, "20220116101530.000000"),
        ]);
        let policy = Policy {
            dates: dates::Policy::Shift,
            ..Policy::default()
        };
        let remap = Remap::new(Root::default(), b"a key of some length");
        let done = apply(&mut o, &plan(&policy, Some(&remap), -10));
        assert_eq!(text(&o, tags::STUDY_DATE).as_deref(), Some("20220105"));
        assert_eq!(text(&o, tags::SERIES_DATE).as_deref(), Some("20220105"));
        assert_eq!(
            text(&o, tags::ACQUISITION_DATE).as_deref(),
            Some("20220106")
        );
        // A datetime keeps its time and moves its date.
        assert_eq!(
            text(&o, tags::ACQUISITION_DATE_TIME).as_deref(),
            Some("20220106101530.000000")
        );
        assert_eq!(done.total("shifted"), 4);
    }

    #[test]
    fn a_year_only_release_writes_the_first_of_january() {
        let mut o = object(&[
            (tags::PATIENT_ID, VR::LO, "x"),
            (tags::STUDY_DATE, VR::DA, "20220715"),
        ]);
        let policy = Policy {
            dates: dates::Policy::Year,
            ..Policy::default()
        };
        let remap = Remap::new(Root::default(), b"a key of some length");
        apply(&mut o, &plan(&policy, Some(&remap), 0));
        assert_eq!(text(&o, tags::STUDY_DATE).as_deref(), Some("20220101"));
    }

    #[test]
    fn a_uid_is_remapped_and_a_standard_one_is_not() {
        // A transfer syntax or a SOP class names a standard, not a study.
        // Remapping one makes the file unreadable.
        let mut o = object(&[
            (tags::PATIENT_ID, VR::LO, "x"),
            (tags::STUDY_INSTANCE_UID, VR::UI, "1.2.3.4"),
            (tags::SERIES_INSTANCE_UID, VR::UI, "1.2.3.5"),
            (tags::SOP_INSTANCE_UID, VR::UI, "1.2.3.6"),
            (tags::SOP_CLASS_UID, VR::UI, "1.2.840.10008.5.1.4.1.1.4"),
        ]);
        let policy = Policy::default();
        let remap = Remap::new(Root::default(), b"a key of some length");
        let done = apply(&mut o, &plan(&policy, Some(&remap), 0));
        assert_eq!(
            text(&o, tags::SOP_CLASS_UID).as_deref(),
            Some("1.2.840.10008.5.1.4.1.1.4"),
            "the class is what says how to read it"
        );
        for tag in [
            tags::STUDY_INSTANCE_UID,
            tags::SERIES_INSTANCE_UID,
            tags::SOP_INSTANCE_UID,
        ] {
            let v = text(&o, tag).unwrap();
            assert!(v.starts_with("2.25."), "{v}");
        }
        assert_eq!(done.total("remapped"), 3);
        // And the meta table agrees with the dataset, or the file disagrees
        // with itself and a reader that trusts one and not the other sees two
        // instances.
        assert_eq!(
            o.meta().media_storage_sop_instance_uid.trim(),
            text(&o, tags::SOP_INSTANCE_UID).unwrap()
        );
    }

    #[test]
    fn preserving_uids_leaves_them_alone() {
        let mut o = object(&[
            (tags::PATIENT_ID, VR::LO, "x"),
            (tags::STUDY_INSTANCE_UID, VR::UI, "1.2.3.4"),
        ]);
        let policy = Policy {
            uids: Uids::Preserve,
            ..Policy::default()
        };
        apply(&mut o, &plan(&policy, None, 0));
        assert_eq!(
            text(&o, tags::STUDY_INSTANCE_UID).as_deref(),
            Some("1.2.3.4")
        );
    }

    #[test]
    fn two_files_of_one_study_get_the_same_new_study_uid() {
        // Which is what makes the output a study rather than a heap.
        let policy = Policy::default();
        let remap = Remap::new(Root::default(), b"a key of some length");
        let mut one = object(&[
            (tags::PATIENT_ID, VR::LO, "x"),
            (tags::STUDY_INSTANCE_UID, VR::UI, "1.2.3.4"),
            (tags::SOP_INSTANCE_UID, VR::UI, "1.2.3.6"),
        ]);
        let mut two = object(&[
            (tags::PATIENT_ID, VR::LO, "x"),
            (tags::STUDY_INSTANCE_UID, VR::UI, "1.2.3.4"),
            (tags::SOP_INSTANCE_UID, VR::UI, "1.2.3.7"),
        ]);
        apply(&mut one, &plan(&policy, Some(&remap), 0));
        apply(&mut two, &plan(&policy, Some(&remap), 0));
        assert_eq!(
            text(&one, tags::STUDY_INSTANCE_UID),
            text(&two, tags::STUDY_INSTANCE_UID)
        );
        assert_ne!(
            text(&one, tags::SOP_INSTANCE_UID),
            text(&two, tags::SOP_INSTANCE_UID)
        );
    }

    #[test]
    fn what_makes_a_file_a_file_is_never_removed() {
        let mut o = object(&[
            (tags::PATIENT_ID, VR::LO, "x"),
            (tags::SOP_CLASS_UID, VR::UI, "1.2.840.10008.5.1.4.1.1.4"),
            (tags::SOP_INSTANCE_UID, VR::UI, "1.2.3.6"),
        ]);
        let policy = Policy::default();
        apply(&mut o, &plan(&policy, None, 0));
        assert!(text(&o, tags::SOP_CLASS_UID).is_some());
        assert!(text(&o, tags::SOP_INSTANCE_UID).is_some());
    }

    #[test]
    fn what_was_changed_is_counted_per_tag_and_never_quoted() {
        // §8.5: an audit that records what was removed is a copy of the
        // identifiers, in clear. What a release removed is recoverable from
        // the originals by someone entitled to read them.
        let mut o = object(&[
            (tags::PATIENT_ID, VR::LO, "19800101-1234"),
            (tags::PATIENT_NAME, VR::PN, "SVENSSON^ANNA"),
            (tags::INSTITUTION_NAME, VR::LO, "Karolinska"),
        ]);
        let policy = Policy::default();
        let done = apply(&mut o, &plan(&policy, None, 0));
        assert_eq!(done.total("removed"), 2);
        let rendered = format!("{:?}", done.changes);
        assert!(!rendered.contains("SVENSSON"), "{rendered}");
        assert!(!rendered.contains("Karolinska"), "{rendered}");
        assert!(rendered.contains("(0010,0010)"), "{rendered}");
    }

    #[test]
    fn the_times_go_and_the_dates_stay_to_be_governed_by_the_policy() {
        let mut o = object(&[
            (tags::PATIENT_ID, VR::LO, "x"),
            (tags::STUDY_DATE, VR::DA, "20220115"),
            (tags::STUDY_TIME, VR::TM, "031415"),
            (tags::SERIES_TIME, VR::TM, "031500"),
        ]);
        let policy = Policy::default();
        apply(&mut o, &plan(&policy, None, 0));
        assert_eq!(text(&o, tags::STUDY_TIME), None, "a scan at 03:14 narrows");
        assert_eq!(text(&o, tags::SERIES_TIME), None);
        assert_eq!(text(&o, tags::STUDY_DATE).as_deref(), Some("20220115"));
    }
}
