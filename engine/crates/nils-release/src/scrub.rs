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
use dicom_core::value::DataSetSequence;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_dictionary_std::tags;
use dicom_object::{DefaultDicomObject, InMemDicomObject};

use crate::dates;
use crate::policy::Policy;
use crate::tags::{Category, MANDATORY};
use crate::uid::Remap;
use nils_registry::day::Day;

/// Which of the two writers applies a plan: the pseudonymiser, writing
/// `dcm-anon`, or a release (spec Wave 7a §6.1). The writer is named in the
/// file's `DeidentificationMethod`, and its own plan decides the options the
/// file says were applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Writer {
    Pseudonymise,
    Release,
}

impl Writer {
    pub fn name(self) -> &'static str {
        match self {
            Writer::Pseudonymise => "pseudonymise",
            Writer::Release => "release",
        }
    }

    /// The text of (0012,0063): NILS, its version and the writer. Short, so
    /// it fits one LO value of 64 characters.
    pub fn method(self) -> String {
        format!("NILS {} {}", env!("CARGO_PKG_VERSION"), self.name())
    }
}

/// What to do to one subject's files.
pub struct Plan<'a> {
    /// Who writes the file, named in its de-identification marks.
    pub writer: Writer,
    pub policy: &'a Policy,
    pub categories: &'a [Category],
    /// The private elements the pack says are worth keeping (§8.4). Everything
    /// else in an odd group goes, and so do the overlays and the curves.
    pub private: &'a [nils_pack::private::Allowed],
    /// The pseudonym this subject's `PatientID` becomes. The registry chose
    /// it; the release does not choose a pseudonym of its own (§8.1).
    pub code: &'a str,
    /// None when the policy preserves UIDs.
    pub remap: Option<&'a Remap>,
    /// Tags kept whatever a category says (record 26 §3: a dataset's `keep`
    /// list, and the demographics it keeps). Keep wins over remove.
    pub keep: &'a [Tag],
    /// Tags removed beside the categories (a dataset's `remove` list).
    pub remove: &'a [Tag],
}

/// What was done, counted per tag so the audit can say so without saying what
/// the value was (§8.5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// Tag to action to count. The action is `removed`, `replaced`, `kept`
    /// or `remapped`; there is deliberately no old value anywhere.
    pub changes: BTreeMap<(String, &'static str), i64>,
    /// The age it wrote, when it could compute one.
    pub age: Option<i64>,
}

impl Applied {
    fn note(&mut self, tag: Tag, action: &'static str) {
        self.count(
            &format!("({:04X},{:04X})", tag.group(), tag.element()),
            action,
            1,
        );
    }

    fn count(&mut self, what: &str, action: &'static str, n: i64) {
        *self.changes.entry((what.to_string(), action)).or_insert(0) += n;
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

    // 2. The declared categories and the named removals, less what makes a
    //    file a file and less what is named to keep.
    for tag in removals(plan) {
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

    // 3b. What the file says about itself (spec Wave 7a §6.1): that it was
    //     de-identified, by which writer, and under which options of the
    //     standard, derived from this plan. Written after the removals, so no
    //     category and no dataset's `remove` list can take them out; never
    //     counted, because they are not a change to anything the file held.
    mark(object, plan);

    // 4. The private blocks, the overlays and the curves, none of which a
    //    list of named standard tags can reach.
    let dropped = crate::blocks::strip(object, plan.private);
    if dropped.overlay > 0 {
        done.count("overlay", "removed", dropped.overlay);
    }
    if dropped.curve > 0 {
        done.count("curve", "removed", dropped.curve);
    }
    for (creator, n) in &dropped.creators {
        done.count(&format!("private {creator}"), "removed", *n);
    }
    for (what, n) in &dropped.kept {
        done.count(what, "kept", *n);
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

/// The elements a plan removes: the declared categories and the named
/// removals, less what makes a file a file, less the age (written, not
/// removed) and the code's element, and less what is named to keep. One
/// statement of it, so that what `apply` removes and what the marks say was
/// kept cannot part company.
pub fn removals(plan: &Plan) -> Vec<Tag> {
    let mut out = crate::tags::tags_of(plan.categories);
    out.extend_from_slice(plan.remove);
    out.sort_unstable();
    out.dedup();
    out.retain(|tag| {
        // The age is written by `apply` and is not an identifier: v0's
        // patient category holds it, which is why v0 cannot both remove the
        // birth date and keep an age.
        !MANDATORY.iter().any(|(g, e)| Tag(*g, *e) == *tag)
            && *tag != tags::PATIENT_AGE
            && *tag != tags::PATIENT_ID
            && !plan.keep.contains(tag)
    });
    out
}

/// One option of DICOM PS3.16 CID 7050, "De-identification Method", as the
/// code sequence (0012,0064) carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deid {
    pub code: &'static str,
    pub meaning: &'static str,
    /// When a writer applies it, in a reader's words.
    pub when: &'static str,
}

/// The coding scheme of every option: DICOM's own.
pub const DEID_SCHEME: &str = "DCM";

pub const BASIC_PROFILE: Deid = Deid {
    code: "113100",
    meaning: "Basic Application Confidentiality Profile",
    when: "always",
};
pub const FULL_DATES: Deid = Deid {
    code: "113106",
    meaning: "Retain Longitudinal Temporal Information Full Dates Option",
    when: "while no date is removed, which is always: the date is the date",
};
pub const PATIENT_CHARACTERISTICS: Deid = Deid {
    code: "113108",
    meaning: "Retain Patient Characteristics Option",
    when: "while the age, sex, size or weight is kept",
};
pub const DEVICE_IDENTITY: Deid = Deid {
    code: "113109",
    meaning: "Retain Device Identity Option",
    when: "while the device serial number or the station name is kept",
};
pub const UIDS: Deid = Deid {
    code: "113110",
    meaning: "Retain UIDs Option",
    when: "while the UIDs are kept, not remapped",
};
pub const SAFE_PRIVATE: Deid = Deid {
    code: "113111",
    meaning: "Retain Safe Private Option",
    when: "while the pack's allowlist keeps private elements",
};
pub const INSTITUTION_IDENTITY: Deid = Deid {
    code: "113112",
    meaning: "Retain Institution Identity Option",
    when: "while the institution's name or address is kept",
};

/// Every option a writer of NILS can state, in code order.
pub const DEID_OPTIONS: [Deid; 7] = [
    BASIC_PROFILE,
    FULL_DATES,
    PATIENT_CHARACTERISTICS,
    DEVICE_IDENTITY,
    UIDS,
    SAFE_PRIVATE,
    INSTITUTION_IDENTITY,
];

/// The dates and datetimes the full-dates option speaks for.
const DATES: [Tag; 8] = [
    tags::STUDY_DATE,
    tags::SERIES_DATE,
    tags::ACQUISITION_DATE,
    tags::CONTENT_DATE,
    tags::INSTANCE_CREATION_DATE,
    tags::ACQUISITION_DATE_TIME,
    tags::PERFORMED_PROCEDURE_STEP_START_DATE,
    tags::PERFORMED_PROCEDURE_STEP_END_DATE,
];

/// The patient characteristics of PS3.15's option that a plan can keep.
const CHARACTERISTICS: [Tag; 4] = [
    tags::PATIENT_AGE,
    tags::PATIENT_SEX,
    tags::PATIENT_SIZE,
    tags::PATIENT_WEIGHT,
];

const DEVICE: [Tag; 2] = [tags::DEVICE_SERIAL_NUMBER, tags::STATION_NAME];

const INSTITUTION: [Tag; 2] = [tags::INSTITUTION_NAME, tags::INSTITUTION_ADDRESS];

/// The options of CID 7050 this plan applies, in code order: derived from
/// the plan and never written by hand, so a change of policy changes the
/// marks (spec Wave 7a §6.1).
pub fn options(plan: &Plan) -> Vec<Deid> {
    let gone = removals(plan);
    let kept = |list: &[Tag]| list.iter().any(|t| !gone.contains(t));
    let mut out = vec![BASIC_PROFILE];
    if DATES.iter().all(|t| !gone.contains(t)) {
        out.push(FULL_DATES);
    }
    if kept(&CHARACTERISTICS) {
        out.push(PATIENT_CHARACTERISTICS);
    }
    if kept(&DEVICE) {
        out.push(DEVICE_IDENTITY);
    }
    if plan.remap.is_none() {
        out.push(UIDS);
    }
    if !plan.private.is_empty() {
        out.push(SAFE_PRIVATE);
    }
    if kept(&INSTITUTION) {
        out.push(INSTITUTION_IDENTITY);
    }
    out
}

/// What (0028,0303) says: every date leaves as it is (record 38 S3), so
/// the longitudinal information is unmodified.
pub const LONGITUDINAL: &str = "UNMODIFIED";

/// Write the four marks of spec Wave 7a §6.1, replacing any a file carried:
/// a release of a pseudonymised file states its own options, not the
/// pseudonymiser's.
fn mark(object: &mut DefaultDicomObject, plan: &Plan) {
    object.put(DataElement::new(
        tags::PATIENT_IDENTITY_REMOVED,
        VR::CS,
        PrimitiveValue::from("YES"),
    ));
    object.put(DataElement::new(
        tags::DEIDENTIFICATION_METHOD,
        VR::LO,
        PrimitiveValue::from(plan.writer.method()),
    ));
    let items: Vec<InMemDicomObject> = options(plan)
        .into_iter()
        .map(|o| {
            InMemDicomObject::from_element_iter([
                DataElement::new(tags::CODE_VALUE, VR::SH, PrimitiveValue::from(o.code)),
                DataElement::new(
                    tags::CODING_SCHEME_DESIGNATOR,
                    VR::SH,
                    PrimitiveValue::from(DEID_SCHEME),
                ),
                DataElement::new(tags::CODE_MEANING, VR::LO, PrimitiveValue::from(o.meaning)),
            ])
        })
        .collect();
    object.put(DataElement::new(
        tags::DEIDENTIFICATION_METHOD_CODE_SEQUENCE,
        VR::SQ,
        DataSetSequence::from(items),
    ));
    object.put(DataElement::new(
        tags::LONGITUDINAL_TEMPORAL_INFORMATION_MODIFIED,
        VR::CS,
        PrimitiveValue::from(LONGITUDINAL),
    ));
}

/// The four marks' tags, which no category and no removal list may hold.
pub const MARKS: [Tag; 4] = [
    tags::PATIENT_IDENTITY_REMOVED,
    tags::DEIDENTIFICATION_METHOD,
    tags::DEIDENTIFICATION_METHOD_CODE_SEQUENCE,
    tags::LONGITUDINAL_TEMPORAL_INFORMATION_MODIFIED,
];

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

    /// Every category, as a plan borrows them. Held against
    /// [`Category::every`] below, so a category added to one and not the
    /// other is a failure rather than a test that quietly stops applying it.
    const ALL: &[Category] = &[
        Category::Patient,
        Category::Trial,
        Category::Provider,
        Category::Institution,
        Category::Times,
        Category::Ids,
    ];

    #[test]
    fn these_tests_apply_every_category() {
        assert_eq!(ALL, Category::every().as_slice());
    }

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

    fn plan<'a>(policy: &'a Policy, remap: Option<&'a Remap>) -> Plan<'a> {
        Plan {
            writer: Writer::Release,
            policy,
            private: &[],
            categories: ALL,
            code: "a1b2c3d4",
            remap,
            keep: &[],
            remove: &[],
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
        apply(&mut o, &plan(&policy, None));
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
        let done = apply(&mut o, &plan(&policy, None));
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
        apply(&mut o, &plan(&policy, None));
        assert_eq!(text(&o, tags::PATIENT_AGE).as_deref(), Some("037Y"));
    }

    #[test]
    fn every_date_is_written_as_it_is() {
        // Record 38 S3: the date is the date, a datetime keeps its date and
        // its time, and nothing counts a date as changed.
        let mut o = object(&[
            (tags::PATIENT_ID, VR::LO, "x"),
            (tags::STUDY_DATE, VR::DA, "20220115"),
            (tags::SERIES_DATE, VR::DA, "20220115"),
            (tags::ACQUISITION_DATE, VR::DA, "20220116"),
            (tags::ACQUISITION_DATE_TIME, VR::DT, "20220116101530.000000"),
        ]);
        let policy = Policy::default();
        let remap = Remap::new(Root::default(), b"a key of some length");
        let done = apply(&mut o, &plan(&policy, Some(&remap)));
        assert_eq!(text(&o, tags::STUDY_DATE).as_deref(), Some("20220115"));
        assert_eq!(text(&o, tags::SERIES_DATE).as_deref(), Some("20220115"));
        assert_eq!(
            text(&o, tags::ACQUISITION_DATE).as_deref(),
            Some("20220116")
        );
        assert_eq!(
            text(&o, tags::ACQUISITION_DATE_TIME).as_deref(),
            Some("20220116101530.000000")
        );
        assert!(
            done.changes.keys().all(|(tag, _)| tag != "(0008,0020)"),
            "{:?}",
            done.changes
        );
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
        let done = apply(&mut o, &plan(&policy, Some(&remap)));
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
        apply(&mut o, &plan(&policy, None));
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
        apply(&mut one, &plan(&policy, Some(&remap)));
        apply(&mut two, &plan(&policy, Some(&remap)));
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
        apply(&mut o, &plan(&policy, None));
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
        let done = apply(&mut o, &plan(&policy, None));
        assert_eq!(done.total("removed"), 2);
        let rendered = format!("{:?}", done.changes);
        assert!(!rendered.contains("SVENSSON"), "{rendered}");
        assert!(!rendered.contains("Karolinska"), "{rendered}");
        assert!(rendered.contains("(0010,0010)"), "{rendered}");
    }

    #[test]
    fn a_named_tag_is_kept_over_its_category_and_another_removed_beside_them() {
        // Record 26 §3: a dataset keeps its demographics and names what
        // else to keep or remove; keep wins.
        let mut o = object(&[
            (tags::PATIENT_ID, VR::LO, "x"),
            (tags::PATIENT_SEX, VR::CS, "F"),
            (tags::PATIENT_WEIGHT, VR::DS, "62"),
            (tags::PATIENT_NAME, VR::PN, "SVENSSON^ANNA"),
            (tags::STATION_NAME, VR::SH, "MR1"),
            (tags::DEVICE_SERIAL_NUMBER, VR::LO, "12345"),
        ]);
        let policy = Policy::default();
        let keep = [tags::PATIENT_SEX, tags::PATIENT_WEIGHT, tags::STATION_NAME];
        let remove = [tags::DEVICE_SERIAL_NUMBER, tags::STATION_NAME];
        let plan = Plan {
            keep: &keep,
            remove: &remove,
            ..plan(&policy, None)
        };
        let done = apply(&mut o, &plan);
        assert_eq!(text(&o, tags::PATIENT_SEX).as_deref(), Some("F"));
        assert_eq!(text(&o, tags::PATIENT_WEIGHT).as_deref(), Some("62"));
        assert_eq!(
            text(&o, tags::STATION_NAME).as_deref(),
            Some("MR1"),
            "keep wins"
        );
        assert_eq!(text(&o, tags::PATIENT_NAME), None);
        assert_eq!(text(&o, tags::DEVICE_SERIAL_NUMBER), None);
        assert_eq!(done.total("removed"), 2);
        assert!(
            done.changes
                .contains_key(&("(0018,1000)".to_string(), "removed"))
        );
    }

    #[test]
    fn the_numbers_the_hospital_put_on_the_examination_go_and_are_counted() {
        // Record 35, finding 1: an accession number is the hospital's own
        // identifier for that examination, so whoever holds it and can reach
        // the hospital's systems undoes everything else the release did. It
        // was in no category, and the change list therefore never mentioned
        // it either.
        let mut o = object(&[
            (tags::PATIENT_ID, VR::LO, "x"),
            (tags::ACCESSION_NUMBER, VR::SH, "A00000001"),
            (tags::DEVICE_SERIAL_NUMBER, VR::LO, "SN00000001"),
            (tags::STUDY_ID, VR::SH, "S0001"),
            (tags::ADMISSION_ID, VR::LO, "V0001"),
        ]);
        let policy = Policy::default();
        let done = apply(&mut o, &plan(&policy, None));
        for tag in [
            tags::ACCESSION_NUMBER,
            tags::DEVICE_SERIAL_NUMBER,
            tags::STUDY_ID,
            tags::ADMISSION_ID,
        ] {
            assert_eq!(text(&o, tag), None, "{tag:?}");
        }
        // And the run says so, so a reader sees the element was handled
        // rather than absent by luck.
        for named in ["(0008,0050)", "(0018,1000)", "(0020,0010)", "(0038,0010)"] {
            assert!(
                done.changes.contains_key(&(named.to_string(), "removed")),
                "{named} is not in {:?}",
                done.changes
            );
        }
    }

    /// The option codes of (0012,0064), in the order the file carries them.
    fn codes(o: &DefaultDicomObject) -> Vec<String> {
        let e = o
            .element(tags::DEIDENTIFICATION_METHOD_CODE_SEQUENCE)
            .expect("the code sequence");
        e.items()
            .expect("items")
            .iter()
            .map(|item| {
                let scheme = item
                    .element(tags::CODING_SCHEME_DESIGNATOR)
                    .unwrap()
                    .to_str()
                    .unwrap();
                assert_eq!(scheme.trim(), "DCM");
                let meaning = item.element(tags::CODE_MEANING).unwrap().to_str().unwrap();
                let code = item
                    .element(tags::CODE_VALUE)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .trim()
                    .to_string();
                let known = DEID_OPTIONS.iter().find(|d| d.code == code).unwrap();
                assert_eq!(meaning.trim(), known.meaning);
                code
            })
            .collect()
    }

    fn marked(o: &DefaultDicomObject, writer: &str) {
        assert_eq!(
            text(o, tags::PATIENT_IDENTITY_REMOVED).as_deref(),
            Some("YES")
        );
        assert_eq!(
            text(o, tags::DEIDENTIFICATION_METHOD),
            Some(format!("NILS {} {writer}", env!("CARGO_PKG_VERSION")))
        );
        assert_eq!(
            text(o, tags::LONGITUDINAL_TEMPORAL_INFORMATION_MODIFIED).as_deref(),
            Some("UNMODIFIED")
        );
    }

    fn identified() -> DefaultDicomObject {
        object(&[
            (tags::PATIENT_ID, VR::LO, "x"),
            (tags::PATIENT_NAME, VR::PN, "SVENSSON^ANNA"),
            (tags::PATIENT_SEX, VR::CS, "F"),
            (tags::PATIENT_BIRTH_DATE, VR::DA, "19800615"),
            (tags::STUDY_DATE, VR::DA, "20220115"),
            (tags::INSTITUTION_NAME, VR::LO, "Somewhere"),
            (tags::DEVICE_SERIAL_NUMBER, VR::LO, "SN1"),
            (tags::STUDY_INSTANCE_UID, VR::UI, "1.2.3.4"),
        ])
    }

    #[test]
    fn a_release_that_remaps_says_so_and_states_no_uids_option() {
        // Spec Wave 7a §6.1: every category removed, the UIDs remapped. The
        // age is computed and kept, the dates kept; nothing else retained.
        let policy = Policy::default();
        let remap = Remap::new(Root::default(), b"a key of some length");
        let mut o = identified();
        apply(&mut o, &plan(&policy, Some(&remap)));
        marked(&o, "release");
        assert_eq!(codes(&o), ["113100", "113106", "113108"]);
    }

    #[test]
    fn a_release_that_keeps_the_uids_and_private_elements_says_both() {
        let policy = Policy {
            uids: Uids::Preserve,
            ..Policy::default()
        };
        let allowed = [nils_pack::private::Allowed {
            creator: "A VENDOR".into(),
            group: 0x0019,
            element: 0x0C,
            why: "a test".into(),
        }];
        let mut o = identified();
        apply(
            &mut o,
            &Plan {
                private: &allowed,
                ..plan(&policy, None)
            },
        );
        marked(&o, "release");
        assert_eq!(
            codes(&o),
            ["113100", "113106", "113108", "113110", "113111"]
        );
    }

    #[test]
    fn the_options_follow_what_the_plan_keeps_and_never_a_hand_list() {
        // The pseudonymiser's four categories leave the device serial number
        // (the ids category is a release's), and a plan that keeps the
        // institution says so; one that removes them both says neither.
        let policy = Policy::default();
        let four = [
            Category::Patient,
            Category::Trial,
            Category::Provider,
            Category::Institution,
        ];
        let keep = [tags::PATIENT_SEX, tags::INSTITUTION_NAME];
        let p = Plan {
            writer: Writer::Pseudonymise,
            categories: &four,
            keep: &keep,
            ..plan(&policy, None)
        };
        let mut o = identified();
        apply(&mut o, &p);
        marked(&o, "pseudonymise");
        assert_eq!(
            codes(&o),
            ["113100", "113106", "113108", "113109", "113110", "113112"]
        );
        assert_eq!(text(&o, tags::DEVICE_SERIAL_NUMBER).as_deref(), Some("SN1"));
        assert_eq!(
            text(&o, tags::INSTITUTION_NAME).as_deref(),
            Some("Somewhere")
        );

        let remove = [tags::DEVICE_SERIAL_NUMBER, tags::STATION_NAME];
        let p = Plan {
            writer: Writer::Pseudonymise,
            categories: &four,
            remove: &remove,
            ..plan(&policy, None)
        };
        let mut o = identified();
        apply(&mut o, &p);
        assert_eq!(codes(&o), ["113100", "113106", "113108", "113110"]);
    }

    #[test]
    fn no_removal_list_strips_the_marks_and_a_file_s_old_marks_are_replaced() {
        // A dataset's `remove` naming the marks, and a file that arrives with
        // another writer's: the file leaves with this writer's, exactly.
        let policy = Policy::default();
        let remap = Remap::new(Root::default(), b"a key of some length");
        let mut o = identified();
        o.put(DataElement::new(
            tags::PATIENT_IDENTITY_REMOVED,
            VR::CS,
            PrimitiveValue::from("NO"),
        ));
        o.put(DataElement::new(
            tags::DEIDENTIFICATION_METHOD,
            VR::LO,
            PrimitiveValue::from("NILS 0.0.0 pseudonymise"),
        ));
        o.put(DataElement::new(
            tags::LONGITUDINAL_TEMPORAL_INFORMATION_MODIFIED,
            VR::CS,
            PrimitiveValue::from("MODIFIED"),
        ));
        let p = Plan {
            remove: &MARKS,
            ..plan(&policy, Some(&remap))
        };
        apply(&mut o, &p);
        marked(&o, "release");
        assert_eq!(codes(&o), ["113100", "113106", "113108"]);
        for tag in MARKS {
            assert!(o.element_opt(tag).unwrap().is_some(), "{tag:?}");
        }
    }

    #[test]
    fn the_method_fits_one_value_of_its_type() {
        // LO holds 64 characters.
        for w in [Writer::Pseudonymise, Writer::Release] {
            assert!(w.method().len() <= 64, "{}", w.method());
        }
    }

    #[test]
    fn the_times_go_and_the_dates_stay() {
        let mut o = object(&[
            (tags::PATIENT_ID, VR::LO, "x"),
            (tags::STUDY_DATE, VR::DA, "20220115"),
            (tags::STUDY_TIME, VR::TM, "031415"),
            (tags::SERIES_TIME, VR::TM, "031500"),
        ]);
        let policy = Policy::default();
        apply(&mut o, &plan(&policy, None));
        assert_eq!(text(&o, tags::STUDY_TIME), None, "a scan at 03:14 narrows");
        assert_eq!(text(&o, tags::SERIES_TIME), None);
        assert_eq!(text(&o, tags::STUDY_DATE).as_deref(), Some("20220115"));
    }
}
