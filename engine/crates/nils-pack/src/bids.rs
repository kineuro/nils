// SPDX-License-Identifier: AGPL-3.0-only

//! How a pack's vocabulary maps onto BIDS
//! (`docs/specs/wave3-anonymize-and-bids.md`, §9.2).
//!
//! The split is the point. The **engine** carries the standard: the entity
//! grammar, which entities a suffix takes and which it requires, and what a
//! label may spell. That is in `nils-release`'s vendored copy of the schema. A
//! **pack** carries the other half: which of its values means `T1w`, which
//! becomes an entity, and which is left to describe the acquisition.
//!
//! So a pack can be wrong about MRI and cannot be wrong about BIDS, which is
//! the only division that makes the mapping editable by the people who know
//! the scanners without making the standard editable at all.
//!
//! v0 has nothing here: its "BIDS export" writes the descriptive name into a
//! folder named by the intent, so no entity name appears anywhere in it.

use std::collections::BTreeMap;

/// What one of our values says a file is called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    /// The BIDS datatype it belongs in, which a suffix fixes: an `ADC` is
    /// `dwi` whatever folder the pack's intent cascade put it in.
    pub datatype: String,
    pub suffix: String,
    /// Only when the technique agrees. `INV1` on anything but an MP2RAGE is
    /// not an MP2RAGE inversion.
    pub when_technique: Option<String>,
    /// Entities the suffix fixes: the first inversion of an MP2RAGE is
    /// `inv-1` and saying so is part of saying it is an `INV1`.
    pub entities: BTreeMap<String, String>,
}

/// One group of `acq-` tokens, from one field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokens {
    /// The axis or fingerprint field the value comes from.
    pub from: String,
    pub tokens: BTreeMap<String, String>,
}

/// The whole mapping.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mapping {
    /// Our intent to the BIDS datatype. An intent absent here has no datatype,
    /// which is not a failure: BIDS has no datatype for a scout.
    pub datatypes: BTreeMap<String, String>,
    /// The four sources of a suffix, consulted in this order.
    pub from_construct: BTreeMap<String, Named>,
    pub from_technique: BTreeMap<String, Named>,
    pub from_modifier: BTreeMap<String, Named>,
    pub from_base: BTreeMap<String, Named>,
    /// Construct to the `part` entity's label.
    pub part: BTreeMap<String, String>,
    /// Modifier to the `mt` entity's label.
    pub mtransfer: BTreeMap<String, String>,
    /// The contrast agent's label, which the archive does not carry in a form
    /// a filename may spell.
    pub ceagent: String,
    /// The `acq-` tokens, **in the order they are joined**.
    pub acq: Vec<Tokens>,
    /// Which provenances and constructs are a vendor's synthetic contrast,
    /// which §9.3 lets a release place in `anat/` or in `derivatives/`.
    pub synthetic_provenance: Vec<String>,
    pub synthetic_construct: Vec<String>,
    /// What a person may answer when asked what the subject was doing, and
    /// why each is on the list.
    pub task: BTreeMap<String, String>,
}

impl Mapping {
    /// Whether the pack declares a BIDS mapping at all. One that does not
    /// cannot be released in the BIDS layout, and says so rather than writing
    /// a tree of stacks it could not name.
    pub fn is_empty(&self) -> bool {
        self.datatypes.is_empty()
    }

    /// The suffix for a stack, from the first source that answers.
    ///
    /// `construct` is plural because the axis is: a stack may say it is both
    /// `Magnitude` and `ADC`, and the first that names a suffix decides.
    pub fn suffix(
        &self,
        constructs: &[&str],
        technique: Option<&str>,
        modifiers: &[&str],
        base: Option<&str>,
    ) -> Option<&Named> {
        for c in constructs {
            if let Some(n) = self.from_construct.get(*c)
                && n.when_technique
                    .as_deref()
                    .is_none_or(|w| technique == Some(w))
            {
                return Some(n);
            }
        }
        if let Some(t) = technique
            && let Some(n) = self.from_technique.get(t)
        {
            return Some(n);
        }
        for m in modifiers {
            if let Some(n) = self.from_modifier.get(*m) {
                return Some(n);
            }
        }
        base.and_then(|b| self.from_base.get(b))
    }

    /// The `part` label a stack's constructs give it, if any.
    pub fn part_of(&self, constructs: &[&str]) -> Option<&str> {
        constructs
            .iter()
            .find_map(|c| self.part.get(*c))
            .map(String::as_str)
    }

    /// The `mt` label a stack's modifiers give it, if any.
    pub fn mtransfer_of(&self, modifiers: &[&str]) -> Option<&str> {
        modifiers
            .iter()
            .find_map(|m| self.mtransfer.get(*m))
            .map(String::as_str)
    }

    /// Whether a stack is a vendor's synthetic contrast (§9.3).
    pub fn is_synthetic(&self, provenance: Option<&str>, constructs: &[&str]) -> bool {
        provenance.is_some_and(|p| self.synthetic_provenance.iter().any(|s| s == p))
            || constructs
                .iter()
                .any(|c| self.synthetic_construct.iter().any(|s| s == c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(datatype: &str, suffix: &str, when: Option<&str>) -> Named {
        Named {
            datatype: datatype.into(),
            suffix: suffix.into(),
            when_technique: when.map(str::to_string),
            entities: BTreeMap::new(),
        }
    }

    fn mapping() -> Mapping {
        let mut m = Mapping::default();
        m.datatypes.insert("anat".into(), "anat".into());
        m.from_construct
            .insert("ADC".into(), named("dwi", "ADC", None));
        m.from_construct
            .insert("INV1".into(), named("anat", "MP2RAGE", Some("MP2RAGE")));
        m.from_technique
            .insert("ME-GRE".into(), named("anat", "MEGRE", None));
        m.from_modifier
            .insert("FLAIR".into(), named("anat", "FLAIR", None));
        m.from_base.insert("T1w".into(), named("anat", "T1w", None));
        m.from_base.insert("T2w".into(), named("anat", "T2w", None));
        m.part.insert("Magnitude".into(), "mag".into());
        m.mtransfer.insert("MT".into(), "on".into());
        m
    }

    #[test]
    fn a_construct_outranks_the_base_contrast() {
        // A stack that says it is an ADC map is an ADC map whatever its base
        // reads, and its datatype comes with the suffix.
        let m = mapping();
        let n = m
            .suffix(&["ADC"], Some("DWI-EPI"), &[], Some("T2w"))
            .unwrap();
        assert_eq!(n.suffix, "ADC");
        assert_eq!(n.datatype, "dwi");
    }

    #[test]
    fn a_qualified_construct_needs_its_technique() {
        // `INV1` on anything but an MP2RAGE is not an MP2RAGE inversion, so
        // the stack falls through to what it otherwise is.
        let m = mapping();
        assert_eq!(
            m.suffix(&["INV1"], Some("MP2RAGE"), &[], None)
                .unwrap()
                .suffix,
            "MP2RAGE"
        );
        assert_eq!(
            m.suffix(&["INV1"], Some("MPRAGE"), &[], Some("T1w"))
                .unwrap()
                .suffix,
            "T1w"
        );
    }

    #[test]
    fn the_four_sources_are_tried_in_order() {
        let m = mapping();
        assert_eq!(
            m.suffix(&[], Some("ME-GRE"), &["FLAIR"], Some("T1w"))
                .unwrap()
                .suffix,
            "MEGRE"
        );
        assert_eq!(
            m.suffix(&[], None, &["FLAIR"], Some("T1w")).unwrap().suffix,
            "FLAIR"
        );
        assert_eq!(m.suffix(&[], None, &[], Some("T1w")).unwrap().suffix, "T1w");
    }

    #[test]
    fn a_value_with_no_bids_word_answers_nothing() {
        // Which is not a failure: it routes the stack to `derivatives/` or
        // reports it. BIDS has no suffix for an SWI image and inventing one
        // would put a claim in a filename.
        let m = mapping();
        assert!(m.suffix(&["SWI"], None, &[], Some("SWI")).is_none());
        assert!(m.suffix(&[], None, &[], None).is_none());
    }

    #[test]
    fn a_synthetic_contrast_is_one_the_pack_names() {
        let mut m = mapping();
        m.synthetic_provenance = vec!["SyMRI".into()];
        m.synthetic_construct = vec!["SyntheticT1w".into()];
        assert!(m.is_synthetic(Some("SyMRI"), &[]));
        assert!(m.is_synthetic(None, &["SyntheticT1w"]));
        assert!(!m.is_synthetic(Some("STAGE"), &["Magnitude"]));
    }

    #[test]
    fn an_entity_comes_from_the_value_that_means_it() {
        let m = mapping();
        assert_eq!(m.part_of(&["ND", "Magnitude"]), Some("mag"));
        assert_eq!(m.part_of(&["ND"]), None);
        assert_eq!(m.mtransfer_of(&["FatSat", "MT"]), Some("on"));
    }
}
