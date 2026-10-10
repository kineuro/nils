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
use crate::tags::{Category, MANDATORY, NEVER_LEAVES};
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
    /// Tag to action to count. The action is `removed`, `replaced`, `kept`,
    /// `remapped` or `cleaned` (a description the file's own identifiers
    /// were taken out of); there is deliberately no old value anywhere.
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

    // The file's own identifiers, read before any of them is removed, for
    // cleaning the descriptions it keeps (step 2b).
    let own = identifiers(object);

    // 2. The declared categories and the named removals, less what makes a
    //    file a file and less what is named to keep.
    for tag in removals(plan) {
        if object.remove_element(tag) {
            done.note(tag, "removed");
        }
    }

    // 2b. The descriptions the rules read stay, cleaned of the file's own
    //     identifiers (PS3.15's Clean Descriptors Option, the review of Wave
    //     7a's merge, 2026-10-10): a name's words, an ID, the accession
    //     number or the birth date typed into a description become `X`.
    for tag in DESCRIPTORS {
        if let Some(text) = text_of(object, tag)
            && let Some(clean) = cleaned(&text, &own)
        {
            let vr = object
                .element_opt(tag)
                .ok()
                .flatten()
                .map(|e| e.vr())
                .unwrap_or(VR::LO);
            object.put(DataElement::new(tag, vr, PrimitiveValue::from(clean)));
            done.note(tag, "cleaned");
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

/// The numbers the hospital's systems put on the examination, the accession
/// number and the study id, which every writer removes from every file it
/// writes, whatever its categories and whatever a dataset names to keep
/// (Nima's ruling of 2026-10-09). Whoever holds either and can reach the
/// hospital's systems is back at the person, so a file that kept one could
/// not say it was de-identified under the basic profile, and every file
/// says so.
///
/// Removed, as their peers are: the basic profile's action for both is to
/// write them empty, since the study module calls them type 2, and every
/// other type 2 identifier a category holds (the patient's name, the
/// referring physician) is removed rather than emptied, so the two are not
/// treated otherwise.
pub const EXAMINATION_IDS: [Tag; 2] = [tags::ACCESSION_NUMBER, tags::STUDY_ID];

/// The elements a plan removes: the declared categories, the named removals
/// and the examination's numbers, less what makes a file a file, less the
/// age (written, not removed) and the code's element, and less what is named
/// to keep, which never holds the examination's numbers. One statement of
/// it, so that what `apply` removes and what the marks say was kept cannot
/// part company.
pub fn removals(plan: &Plan) -> Vec<Tag> {
    let mut out = crate::tags::tags_of(plan.categories);
    out.extend_from_slice(plan.remove);
    out.extend_from_slice(&EXAMINATION_IDS);
    out.sort_unstable();
    out.dedup();
    out.retain(|tag| {
        // The age is written by `apply` and is not an identifier: v0's
        // patient category holds it, which is why v0 cannot both remove the
        // birth date and keep an age.
        !MANDATORY.iter().any(|(g, e)| Tag(*g, *e) == *tag)
            && *tag != tags::PATIENT_AGE
            && *tag != tags::PATIENT_ID
            && (EXAMINATION_IDS.contains(tag) || unkeepable(*tag) || !plan.keep.contains(tag))
    });
    out
}

/// The direct identifiers no option of the standard retains: what may never
/// survive (`NEVER_LEAVES`) less the device and the institution, which the
/// device and institution identity options retain by name, and less the
/// examination's numbers, which go whatever a list says. A dataset's or a
/// release's `keep` never holds one (the review of Wave 7a's merge,
/// 2026-10-10): a list that kept the patient's name would leave a file that
/// says its identity was removed.
pub fn unkeepable(tag: Tag) -> bool {
    NEVER_LEAVES.iter().any(|(g, e)| Tag(*g, *e) == tag)
        && !DEVICE.contains(&tag)
        && !INSTITUTION.contains(&tag)
        && !EXAMINATION_IDS.contains(&tag)
}

/// Whether a plan meets the basic profile: every direct identifier NILS
/// lists (`NEVER_LEAVES`) and every element of the `ids` category is
/// removed, retained under an option the plan claims (the device's, the
/// institution's), or a description the plan cleans. Derived from the plan,
/// so the claim and the removals cannot part company.
pub fn basic_profile_holds(plan: &Plan) -> bool {
    let gone = removals(plan);
    let claimed_device = DEVICE.iter().any(|t| !gone.contains(t));
    let claimed_institution = INSTITUTION.iter().any(|t| !gone.contains(t));
    NEVER_LEAVES
        .iter()
        .chain(Category::Ids.tags())
        .map(|(g, e)| Tag(*g, *e))
        .all(|tag| {
            gone.contains(&tag)
                || DESCRIPTORS.contains(&tag)
                || (claimed_device && DEVICE.contains(&tag))
                || (claimed_institution && INSTITUTION.contains(&tag))
        })
}

/// The descriptions every writer keeps because the rules read them (the
/// study's and the series's descriptions, the protocol's name, the image's
/// comments, the contrast agent), each cleaned of the file's own identifiers
/// before it leaves: PS3.15's Clean Descriptors Option.
pub const DESCRIPTORS: [Tag; 5] = [
    tags::STUDY_DESCRIPTION,
    tags::SERIES_DESCRIPTION,
    tags::PROTOCOL_NAME,
    tags::IMAGE_COMMENTS,
    tags::CONTRAST_BOLUS_AGENT,
];

/// The file's own identifiers as a description might repeat them: every
/// value of its IDs, accession number, study id, admission id and order
/// numbers, and its birth date, as written and as digits alone (a twelve
/// digit number's last ten too), four letters or digits at the least; and
/// the words of its names, three letters at the least. Lower case, the
/// longest first.
#[derive(Debug, Default)]
pub struct Identifiers {
    values: Vec<Vec<char>>,
    words: Vec<Vec<char>>,
}

/// One character folded to lower case, one for one, so a folded text keeps
/// the positions of the text it was folded from.
fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn folded(text: &str) -> Vec<char> {
    text.chars().map(fold).collect()
}

/// The identifiers of one file, read before anything is removed.
pub fn identifiers(object: &InMemDicomObject) -> Identifiers {
    let mut values: Vec<Vec<char>> = Vec::new();
    for tag in [
        tags::PATIENT_ID,
        Tag(0x0010, 0x1000),
        tags::ACCESSION_NUMBER,
        tags::STUDY_ID,
        tags::ADMISSION_ID,
        tags::PLACER_ORDER_NUMBER_IMAGING_SERVICE_REQUEST,
        tags::FILLER_ORDER_NUMBER_IMAGING_SERVICE_REQUEST,
        tags::PATIENT_BIRTH_DATE,
    ] {
        let Some(text) = text_of(object, tag) else {
            continue;
        };
        for value in text.split('\\').map(str::trim) {
            if value.chars().filter(|c| c.is_alphanumeric()).count() < 4 {
                continue;
            }
            values.push(folded(value));
            let digits: String = value.chars().filter(char::is_ascii_digit).collect();
            if digits.len() >= 6 && digits != value {
                values.push(folded(&digits));
            }
            // a personnummer's other ways of being written: without its
            // century, and with the hyphen before the last four
            if digits.len() == 12 {
                values.push(folded(&digits[2..]));
                values.push(folded(&format!("{}-{}", &digits[..8], &digits[8..])));
                values.push(folded(&format!("{}-{}", &digits[2..8], &digits[8..])));
            }
            if digits.len() == 10 {
                values.push(folded(&format!("{}-{}", &digits[..6], &digits[6..])));
            }
        }
    }
    let mut words: Vec<Vec<char>> = Vec::new();
    for tag in [
        tags::PATIENT_NAME,
        tags::OTHER_PATIENT_NAMES,
        tags::PATIENT_BIRTH_NAME,
        tags::PATIENT_MOTHER_BIRTH_NAME,
    ] {
        let Some(text) = text_of(object, tag) else {
            continue;
        };
        for word in text.split(|c: char| !c.is_alphanumeric()) {
            if word.chars().filter(|c| c.is_alphabetic()).count() >= 3 {
                words.push(folded(word));
            }
        }
    }
    for list in [&mut values, &mut words] {
        list.sort_unstable();
        list.dedup();
        list.sort_by_key(|v| std::cmp::Reverse(v.len()));
    }
    Identifiers { values, words }
}

/// A description with each whole occurrence of the file's identifiers put
/// as `X`: a value where no letter or digit adjoins it, a name's word where
/// no letter adjoins it, whatever the case. None when it holds none.
pub fn cleaned(text: &str, own: &Identifiers) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars.iter().copied().map(fold).collect();
    let mut hit = vec![false; chars.len()];
    let mut mark = |needle: &[char], adjoins: fn(char) -> bool| {
        if needle.is_empty() || needle.len() > lower.len() {
            return;
        }
        for i in 0..=lower.len() - needle.len() {
            let end = i + needle.len();
            if lower[i..end] == *needle
                && (i == 0 || !adjoins(lower[i - 1]))
                && (end == lower.len() || !adjoins(lower[end]))
            {
                hit[i..end].iter_mut().for_each(|h| *h = true);
            }
        }
    };
    for value in &own.values {
        mark(value, char::is_alphanumeric);
    }
    for word in &own.words {
        mark(word, char::is_alphabetic);
    }
    if !hit.contains(&true) {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if hit[i] {
            out.push('X');
            while i < chars.len() && hit[i] {
                i += 1;
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    Some(out)
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
    when: "while every direct identifier and every element of the ids category is removed, retained under a claimed option or cleaned as a description: always under the default plans",
};
pub const CLEAN_DESCRIPTORS: Deid = Deid {
    code: "113105",
    meaning: "Clean Descriptors Option",
    when: "always: the descriptions the rules read stay, cleaned of the file's own IDs, accession number, study id, birth date and the words of its names",
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
    when: "while the device's identity is kept (its station name, serial number, UID, gantry or unique device identifier): by the pseudonymiser, since the rules and a decision for this scanner read it",
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
pub const DEID_OPTIONS: [Deid; 8] = [
    BASIC_PROFILE,
    CLEAN_DESCRIPTORS,
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

/// The device's own identity, which the Retain Device Identity Option keeps:
/// its station name, serial number, UID, gantry and unique device
/// identifier.
pub const DEVICE: [Tag; 6] = [
    tags::STATION_NAME,
    tags::DEVICE_SERIAL_NUMBER,
    tags::DEVICE_UID,
    tags::GANTRY_ID,
    Tag(0x0018, 0x1009),
    Tag(0x0018, 0x100A),
];

const INSTITUTION: [Tag; 2] = [tags::INSTITUTION_NAME, tags::INSTITUTION_ADDRESS];

/// The options of CID 7050 this plan applies, in code order: derived from
/// the plan and never written by hand, so a change of policy changes the
/// marks (spec Wave 7a §6.1).
pub fn options(plan: &Plan) -> Vec<Deid> {
    let gone = removals(plan);
    let kept = |list: &[Tag]| list.iter().any(|t| !gone.contains(t));
    let mut out = Vec::new();
    // Never claimed by a plan that leaves a direct identifier or an element
    // of the ids category in the file outside a claimed option, the
    // examination's numbers among them (the review of Wave 7a's merge,
    // 2026-10-10: the claim had followed the examination's numbers alone).
    if EXAMINATION_IDS.iter().all(|t| gone.contains(t)) && basic_profile_holds(plan) {
        out.push(BASIC_PROFILE);
    }
    // every writer cleans the descriptions it keeps (`apply`, step 2b)
    out.push(CLEAN_DESCRIPTORS);
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
        assert_eq!(codes(&o), ["113100", "113105", "113106", "113108"]);
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
            ["113100", "113105", "113106", "113108", "113110", "113111"]
        );
    }

    #[test]
    fn the_options_follow_what_the_plan_keeps_and_never_a_hand_list() {
        // v0's four categories leave the device serial number and the rest
        // of the ids category in the file, so such a plan claims no basic
        // profile (the review of Wave 7a's merge, 2026-10-10: it had claimed
        // one); a plan that keeps the institution says so; one that removes
        // the device and the station says neither.
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
            ["113105", "113106", "113108", "113109", "113110", "113112"]
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
        // the device's UID and gantry, which v0's four do not name, stay
        assert_eq!(
            codes(&o),
            ["113105", "113106", "113108", "113109", "113110"]
        );
        // with the ids category removed beside them, the basic profile holds
        let five = [
            Category::Patient,
            Category::Trial,
            Category::Provider,
            Category::Institution,
            Category::Ids,
        ];
        let p = Plan {
            writer: Writer::Pseudonymise,
            categories: &five,
            keep: &keep,
            ..plan(&policy, None)
        };
        let mut o = identified();
        apply(&mut o, &p);
        assert_eq!(
            codes(&o),
            ["113100", "113105", "113106", "113108", "113110", "113112"]
        );
        assert_eq!(text(&o, tags::DEVICE_SERIAL_NUMBER), None);
    }

    #[test]
    fn the_examination_s_numbers_go_from_every_file_whatever_is_kept() {
        // Nima's ruling of 2026-10-09: the accession number and the study id
        // are removed whenever a file is de-identified, and a dataset's
        // `keep` naming both keeps neither; the file says the basic profile,
        // and it is true.
        let policy = Policy::default();
        let five = [
            Category::Patient,
            Category::Trial,
            Category::Provider,
            Category::Institution,
            Category::Ids,
        ];
        let keep = [tags::ACCESSION_NUMBER, tags::STUDY_ID, tags::PATIENT_SEX];
        for keep in [&keep[..], &[][..]] {
            let p = Plan {
                writer: Writer::Pseudonymise,
                categories: &five,
                keep,
                ..plan(&policy, None)
            };
            let mut o = identified();
            o.put(DataElement::new(
                tags::ACCESSION_NUMBER,
                VR::SH,
                PrimitiveValue::from("A00000001"),
            ));
            o.put(DataElement::new(
                tags::STUDY_ID,
                VR::SH,
                PrimitiveValue::from("S0001"),
            ));
            let done = apply(&mut o, &p);
            for tag in EXAMINATION_IDS {
                assert!(o.element_opt(tag).unwrap().is_none(), "{tag:?}");
            }
            for named in ["(0008,0050)", "(0020,0010)"] {
                assert_eq!(
                    done.changes.get(&(named.to_string(), "removed")),
                    Some(&1),
                    "{named}"
                );
            }
            assert_eq!(codes(&o)[0], "113100");
        }
        // And for a release whatever categories it picks.
        let p = Plan {
            categories: &[Category::Times],
            ..plan(&policy, None)
        };
        assert!(EXAMINATION_IDS.iter().all(|t| removals(&p).contains(t)));
        // which keeps the patient's name and so claims no basic profile
        assert!(!options(&p).contains(&BASIC_PROFILE));
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
        assert_eq!(codes(&o), ["113100", "113105", "113106", "113108"]);
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
    /// A file holding every direct identifier and every element of the ids
    /// category, each with a value.
    fn every_identifier() -> DefaultDicomObject {
        let mut pairs: Vec<(Tag, VR, String)> = NEVER_LEAVES
            .iter()
            .chain(Category::Ids.tags())
            .map(|(g, e)| (Tag(*g, *e), VR::LO, format!("value {g:04X}{e:04X}")))
            .collect();
        pairs.sort_by_key(|(t, _, _)| *t);
        pairs.dedup_by_key(|(t, _, _)| *t);
        let mut o = identified();
        for (tag, vr, value) in pairs {
            o.put(DataElement::new(tag, vr, PrimitiveValue::from(value)));
        }
        o
    }

    #[test]
    fn the_basic_profile_is_claimed_exactly_when_the_file_holds_no_identifier() {
        // The review of Wave 7a's merge (2026-10-10): the claim followed the
        // examination's numbers alone. Now it is compared with the file: a
        // plan that claims the basic profile leaves no direct identifier and
        // no element of the ids category, unless an option it claims retains
        // it or it is a description the plan cleans; a plan that leaves one
        // claims none.
        let policy = Policy::default();
        let four = [
            Category::Patient,
            Category::Trial,
            Category::Provider,
            Category::Institution,
        ];
        let five = [
            Category::Patient,
            Category::Trial,
            Category::Provider,
            Category::Institution,
            Category::Ids,
        ];
        let kept_comments = [tags::IMAGE_COMMENTS, tags::PATIENT_SEX];
        let plans = [
            plan(&policy, None),
            Plan {
                categories: &four,
                ..plan(&policy, None)
            },
            Plan {
                categories: &five,
                ..plan(&policy, None)
            },
            Plan {
                categories: &five,
                keep: &kept_comments,
                ..plan(&policy, None)
            },
            Plan {
                categories: &[Category::Times],
                ..plan(&policy, None)
            },
        ];
        for p in &plans {
            let mut o = every_identifier();
            apply(&mut o, p);
            let claimed = codes(&o).contains(&"113100".to_string());
            let left: Vec<Tag> = NEVER_LEAVES
                .iter()
                .chain(Category::Ids.tags())
                .map(|(g, e)| Tag(*g, *e))
                .filter(|t| o.element_opt(*t).ok().flatten().is_some())
                .filter(|t| !DESCRIPTORS.contains(t))
                .filter(|t| {
                    !(codes(&o).contains(&"113109".to_string()) && DEVICE.contains(t))
                        && !(codes(&o).contains(&"113112".to_string()) && INSTITUTION.contains(t))
                })
                .collect();
            assert_eq!(
                claimed,
                left.is_empty(),
                "{:?}: left {left:?}",
                p.categories
            );
        }
    }

    #[test]
    fn a_dataset_s_keep_never_holds_the_person_s_direct_identifiers() {
        // A `keep` naming the patient's name, birth date and address keeps
        // none of them, so the file can say its identity was removed; the
        // device and the institution stay keepable, under their options.
        let policy = Policy::default();
        let keep = [
            tags::PATIENT_NAME,
            tags::PATIENT_BIRTH_DATE,
            tags::PATIENT_ADDRESS,
            tags::PATIENT_SEX,
            tags::INSTITUTION_NAME,
        ];
        let p = Plan {
            keep: &keep,
            ..plan(&policy, None)
        };
        let mut o = identified();
        o.put(DataElement::new(
            tags::PATIENT_ADDRESS,
            VR::LO,
            PrimitiveValue::from("Storgatan 1"),
        ));
        apply(&mut o, &p);
        assert_eq!(text(&o, tags::PATIENT_NAME), None);
        assert_eq!(text(&o, tags::PATIENT_BIRTH_DATE), None);
        assert_eq!(text(&o, tags::PATIENT_ADDRESS), None);
        assert_eq!(text(&o, tags::PATIENT_SEX).as_deref(), Some("F"));
        assert_eq!(
            text(&o, tags::INSTITUTION_NAME).as_deref(),
            Some("Somewhere")
        );
        assert!(codes(&o).contains(&"113100".to_string()));
        assert!(codes(&o).contains(&"113112".to_string()));
        for tag in [
            tags::PATIENT_NAME,
            tags::PATIENT_BIRTH_DATE,
            tags::PATIENT_ADDRESS,
        ] {
            assert!(unkeepable(tag), "{tag:?}");
        }
        assert!(!unkeepable(tags::INSTITUTION_NAME));
        assert!(!unkeepable(tags::DEVICE_SERIAL_NUMBER));
    }

    #[test]
    fn the_registry_refuses_in_keep_what_no_option_retains() {
        // The registry's own list, which a declaration's `keep` is checked
        // against, is this crate's `NEVER_LEAVES` less the device and the
        // institution, tag for tag.
        let mut ours: Vec<String> = NEVER_LEAVES
            .iter()
            .map(|(g, e)| Tag(*g, *e))
            .filter(|t| unkeepable(*t))
            .map(|t| format!("{:04X},{:04X}", t.group(), t.element()))
            .collect();
        ours.sort();
        let mut theirs: Vec<String> = nils_registry::place::UNKEEPABLE
            .iter()
            .map(|t| t.to_string())
            .collect();
        theirs.sort();
        assert_eq!(ours, theirs);
    }

    #[test]
    fn a_description_loses_the_file_s_own_identifiers_and_nothing_else() {
        // PS3.15's Clean Descriptors Option, as every writer applies it
        // (the review of Wave 7a's merge, 2026-10-10).
        let o = object(&[
            (tags::PATIENT_NAME, VR::PN, "SVENSSON^ANNA"),
            (tags::PATIENT_ID, VR::LO, "191212121212"),
            (tags::ACCESSION_NUMBER, VR::SH, "ACC0042"),
            (tags::PATIENT_BIRTH_DATE, VR::DA, "19121212"),
            (tags::STUDY_ID, VR::SH, "7"),
        ]);
        let own = identifiers(&o);
        assert_eq!(
            cleaned("t1_mprage svensson post", &own).as_deref(),
            Some("t1_mprage X post")
        );
        assert_eq!(cleaned("ACC0042_t2", &own).as_deref(), Some("X_t2"));
        assert_eq!(
            cleaned("pn 121212-1212 done", &own).as_deref(),
            Some("pn X done")
        );
        assert_eq!(cleaned("born 19121212", &own).as_deref(), Some("born X"));
        // a word inside another word, a short id and a description with none
        // of them stay as they are
        assert_eq!(cleaned("svenssonska", &own), None);
        assert_eq!(cleaned("t2_tse 7mm", &own), None);
        assert_eq!(cleaned("t2_tirm_tra_dark-fluid", &own), None);
        // and a release's file: the series description cleaned, counted
        let policy = Policy::default();
        let mut o = identified();
        o.put(DataElement::new(
            tags::SERIES_DESCRIPTION,
            VR::LO,
            PrimitiveValue::from("T1 ANNA SVENSSON"),
        ));
        let done = apply(&mut o, &plan(&policy, None));
        assert_eq!(
            text(&o, tags::SERIES_DESCRIPTION).as_deref(),
            Some("T1 X X")
        );
        assert_eq!(
            done.changes.get(&("(0008,103E)".to_string(), "cleaned")),
            Some(&1)
        );
    }
}
