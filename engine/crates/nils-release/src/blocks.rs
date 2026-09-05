// SPDX-License-Identifier: AGPL-3.0-only

//! Private elements, overlays and curves
//! (`docs/specs/wave3-anonymize-and-bids.md`, §8.4).
//!
//! Three groups of elements that a list of named tags can never cover, because
//! what is in them is not fixed by the standard:
//!
//! - a **private** element means whatever its vendor decided, and nothing in
//!   the file says what;
//! - an **overlay** is a bitmap drawn over the image, and what is drawn on it
//!   is frequently a name, an accession number or an arrow somebody added at a
//!   workstation;
//! - a **curve** is the retired equivalent, and still present in old archives.
//!
//! So all three go, and the private ones come back only by name. v0 removes
//! 119 named standard tags and touches none of these.

use std::collections::BTreeMap;

use dicom_core::Tag;
use dicom_core::header::Header as _;
use dicom_object::{DefaultDicomObject, InMemDicomObject};
use nils_pack::private::Allowed;

/// The first and last group of the overlay range. Even groups only, and the
/// odd ones in between are ordinary private groups.
const OVERLAY: (u16, u16) = (0x6000, 0x60FF);

/// The retired curve range, same shape.
const CURVE: (u16, u16) = (0x5000, 0x50FF);

/// Which private blocks a file declares, as block offset to creator.
///
/// A creator reserves a block by writing its name at `(gggg,00xx)`, and its
/// elements then live at `(gggg,xxee)`. The same vendor lands in a different
/// block from file to file, which is why an allowlist addresses a creator and
/// not a position.
fn creators(object: &InMemDicomObject, group: u16) -> BTreeMap<u16, String> {
    let mut out = BTreeMap::new();
    for e in object.iter() {
        let tag = e.tag();
        if tag.group() != group {
            continue;
        }
        let slot = tag.element();
        if !(0x0010..=0x00FF).contains(&slot) {
            continue;
        }
        if let Ok(name) = e.value().to_str() {
            out.insert(slot, name.trim_matches([' ', '\0']).to_string());
        }
    }
    out
}

/// What was dropped, by what it was.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dropped {
    pub private: i64,
    pub overlay: i64,
    pub curve: i64,
    /// The creators whose blocks were dropped, so a report can say which
    /// vendors an archive carries without saying what they held.
    pub creators: BTreeMap<String, i64>,
    /// And the ones the allowlist kept, by their text.
    pub kept: BTreeMap<String, i64>,
}

/// Drop every private, overlay and curve element the allowlist does not name.
pub fn strip(object: &mut DefaultDicomObject, allowed: &[Allowed]) -> Dropped {
    let mut done = Dropped::default();

    // The creators of every private group, read before anything is removed:
    // dropping a creator element first would orphan the block it names.
    let groups: Vec<u16> = {
        let mut g: Vec<u16> = object
            .iter()
            .map(|e| e.tag().group())
            .filter(|g| g % 2 == 1 && *g > 0x0008)
            .collect();
        g.sort_unstable();
        g.dedup();
        g
    };
    let mut blocks: BTreeMap<u16, BTreeMap<u16, String>> = BTreeMap::new();
    for group in &groups {
        blocks.insert(*group, creators(object, *group));
    }

    let mut remove: Vec<Tag> = Vec::new();
    for e in object.iter() {
        let tag = e.tag();
        let (group, element) = (tag.group(), tag.element());
        if (OVERLAY.0..=OVERLAY.1).contains(&group) && group % 2 == 0 {
            remove.push(tag);
            done.overlay += 1;
            continue;
        }
        if (CURVE.0..=CURVE.1).contains(&group) && group % 2 == 0 {
            remove.push(tag);
            done.curve += 1;
            continue;
        }
        if group % 2 == 0 || group <= 0x0008 {
            continue;
        }
        // The creator element itself: kept only if the block it names has
        // something kept in it, which is settled in a second pass below.
        if (0x0010..=0x00FF).contains(&element) {
            continue;
        }
        let slot = element >> 8;
        let offset = (element & 0x00FF) as u8;
        let creator = blocks
            .get(&group)
            .and_then(|b| b.get(&slot))
            .cloned()
            .unwrap_or_default();
        let allowed = allowed.iter().find(|a| {
            a.group == group
                && a.element == offset
                && a.creator.trim().eq_ignore_ascii_case(creator.trim())
        });
        match allowed {
            Some(a) => {
                *done.kept.entry(a.text()).or_insert(0) += 1;
            }
            None => {
                remove.push(tag);
                done.private += 1;
                let named = if creator.is_empty() {
                    format!("({group:04X},xx) with no creator")
                } else {
                    creator.clone()
                };
                *done.creators.entry(named).or_insert(0) += 1;
            }
        }
    }
    for tag in remove {
        object.remove_element(tag);
    }

    // A creator element whose block is now empty names nothing, so it goes
    // too. One that still reserves a kept element stays, or a reader cannot
    // tell whose the kept element is.
    let mut orphans: Vec<Tag> = Vec::new();
    for (group, block) in &blocks {
        for slot in block.keys() {
            let still = object
                .iter()
                .any(|e| e.tag().group() == *group && (e.tag().element() >> 8) == *slot);
            if !still {
                orphans.push(Tag(*group, *slot));
            }
        }
    }
    for tag in orphans {
        object.remove_element(tag);
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;
    use dicom_core::{DataElement, PrimitiveValue, VR};
    use dicom_object::FileMetaTableBuilder;

    fn object(pairs: &[(u16, u16, VR, &str)]) -> DefaultDicomObject {
        let mut ds = InMemDicomObject::new_empty();
        ds.put(DataElement::new(
            dicom_dictionary_std::tags::SOP_INSTANCE_UID,
            VR::UI,
            PrimitiveValue::from("1.2.3"),
        ));
        for (g, e, vr, v) in pairs {
            ds.put(DataElement::new(Tag(*g, *e), *vr, PrimitiveValue::from(*v)));
        }
        ds.with_meta(
            FileMetaTableBuilder::new()
                .transfer_syntax("1.2.840.10008.1.2.1")
                .media_storage_sop_class_uid("1.2.840.10008.5.1.4.1.1.4")
                .media_storage_sop_instance_uid("1.2.3"),
        )
        .expect("a meta table")
    }

    fn allow(creator: &str, group: u16, element: u8) -> Allowed {
        Allowed {
            creator: creator.into(),
            group,
            element,
            why: "a test".into(),
        }
    }

    fn has(o: &DefaultDicomObject, g: u16, e: u16) -> bool {
        o.element_opt(Tag(g, e)).ok().flatten().is_some()
    }

    #[test]
    fn a_private_element_goes_unless_it_is_named() {
        let mut o = object(&[
            (0x0019, 0x0010, VR::LO, "SIEMENS MR HEADER"),
            (0x0019, 0x100C, VR::IS, "1000"),
            (0x0019, 0x1099, VR::LO, "whatever the vendor felt like"),
        ]);
        let done = strip(&mut o, &[allow("SIEMENS MR HEADER", 0x0019, 0x0C)]);
        assert!(has(&o, 0x0019, 0x100C), "the b value is named and stays");
        assert!(!has(&o, 0x0019, 0x1099), "the rest goes");
        assert_eq!(done.private, 1);
        assert_eq!(done.kept.len(), 1);
    }

    #[test]
    fn the_block_moves_and_the_allowlist_follows_it() {
        // The same vendor lands at a different offset from file to file, which
        // is why an allowlist addresses a creator and not a position. v0's
        // reader takes the fixed slot and reads whatever is there.
        let mut o = object(&[
            (0x0019, 0x0011, VR::LO, "SIEMENS MR HEADER"),
            (0x0019, 0x110C, VR::IS, "1000"),
        ]);
        strip(&mut o, &[allow("SIEMENS MR HEADER", 0x0019, 0x0C)]);
        assert!(has(&o, 0x0019, 0x110C));
    }

    #[test]
    fn another_vendor_in_the_slot_the_allowlist_names_is_not_kept() {
        // The whole reason the creator is the address: at the fixed slot this
        // would be read, and kept, as a Siemens b value.
        let mut o = object(&[
            (0x0019, 0x0010, VR::LO, "SOMEBODY ELSE"),
            (0x0019, 0x100C, VR::LO, "the operator's name"),
        ]);
        let done = strip(&mut o, &[allow("SIEMENS MR HEADER", 0x0019, 0x0C)]);
        assert!(!has(&o, 0x0019, 0x100C));
        assert_eq!(done.private, 1);
        assert_eq!(done.creators.get("SOMEBODY ELSE"), Some(&1));
    }

    #[test]
    fn a_creator_whose_block_is_emptied_goes_with_it() {
        let mut o = object(&[
            (0x0019, 0x0010, VR::LO, "SIEMENS MR HEADER"),
            (0x0019, 0x1099, VR::LO, "something"),
        ]);
        strip(&mut o, &[]);
        assert!(!has(&o, 0x0019, 0x0010), "it names nothing now");
    }

    #[test]
    fn a_creator_that_still_reserves_something_stays() {
        // Or a reader cannot tell whose the kept element is.
        let mut o = object(&[
            (0x0019, 0x0010, VR::LO, "SIEMENS MR HEADER"),
            (0x0019, 0x100C, VR::IS, "1000"),
            (0x0019, 0x1099, VR::LO, "something"),
        ]);
        strip(&mut o, &[allow("SIEMENS MR HEADER", 0x0019, 0x0C)]);
        assert!(has(&o, 0x0019, 0x0010));
    }

    #[test]
    fn an_overlay_goes_because_of_what_is_drawn_on_it() {
        // Frequently a name, an accession number, or an arrow somebody added
        // at a workstation. A list of named tags can never cover it: there are
        // 128 overlay groups and every element of each.
        let mut o = object(&[
            (0x6000, 0x0010, VR::US, "512"),
            (0x6000, 0x3000, VR::OW, "the bitmap"),
            (0x6002, 0x3000, VR::OW, "a second one"),
        ]);
        let done = strip(&mut o, &[]);
        assert!(!has(&o, 0x6000, 0x3000));
        assert!(!has(&o, 0x6002, 0x3000));
        assert_eq!(done.overlay, 3);
    }

    #[test]
    fn a_curve_goes_too_because_old_archives_still_have_them() {
        let mut o = object(&[(0x5000, 0x3000, VR::OW, "a curve")]);
        assert_eq!(strip(&mut o, &[]).curve, 1);
        assert!(!has(&o, 0x5000, 0x3000));
    }

    #[test]
    fn an_odd_group_in_the_overlay_range_is_an_ordinary_private_group() {
        let mut o = object(&[(0x6001, 0x1000, VR::LO, "a private element")]);
        let done = strip(&mut o, &[]);
        assert_eq!(done.overlay, 0);
        assert_eq!(done.private, 1);
    }

    #[test]
    fn the_report_names_the_vendors_and_never_what_they_held() {
        let mut o = object(&[
            (0x0029, 0x0010, VR::LO, "SIEMENS CSA HEADER"),
            (0x0029, 0x1010, VR::OB, "the operator was Anna"),
        ]);
        let done = strip(&mut o, &[]);
        let rendered = format!("{done:?}");
        assert!(rendered.contains("SIEMENS CSA HEADER"), "{rendered}");
        assert!(!rendered.contains("Anna"), "{rendered}");
    }
}
