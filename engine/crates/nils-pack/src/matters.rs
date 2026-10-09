// SPDX-License-Identifier: AGPL-3.0-only

//! Record 55 H3 (2026-10-09): the axes that matter, and so the axes where a
//! stack with no answer is a question.
//!
//! Nima's ruling: the pack decides, and Review is only where it is truly
//! necessary. A conflict between the pack's rules, or a rule's low confidence,
//! is never a question. What stays one, until System 1 answers it, is an
//! axis that matters and that no rule answered. An axis matters when what
//! NILS does with a stack reads it:
//!
//! 1. **the name a release gives it**: the descriptive name and layout, whose
//!    readers are the release code's own (listed in [`RELEASE_READS`], and a
//!    test in `nils-release` reads its source to keep the list true), and the
//!    BIDS name, whose readers the pack declares in its mapping (`bids.yml`:
//!    the four sources of a suffix, the entities, the `acq-` groups, the
//!    folders and the derivatives);
//! 2. **the main scans**: every name a pick model reads ([`Pick::reads`]),
//!    and the `role` axis the pick run finds its candidates by;
//! 3. **a release**: the disposition, which decides whether a stack is
//!    released as an acquisition, a derivative or not at all, the folder its
//!    intent and provenance give it ([`RELEASE_READS`] again).
//!
//! What is left out, and why. The step that separates two stacks whose names
//! collide reads every axis, but only to tell apart names that already
//! collide: an axis with no value separates nothing there, so it does not
//! make the axis matter. A field of the fingerprint (orientation, the
//! acquisition type, a number) is measured, not answered, so it is no axis.
//!
//! An axis that matters raises `<axis>:missing` only where an answer can be
//! missing: it holds one value (an empty multi-valued axis says "none", which
//! is an answer), it has no default (a default is the pack's answer), and the
//! pack does not leave it to an image model (`review.by_model`: the body part,
//! whose model asks in its own run). A rule that decides an axis to nothing
//! answered it too.
//!
//! [`Pick::reads`]: crate::pick::Model::reads

use std::collections::BTreeMap;

use crate::pack::Pack;

/// The axes the release code itself reads by name, beside what the pack's
/// BIDS mapping declares, and where. `nils-release` keeps a test that reads
/// its own source for every axis it reads and fails when this list and the
/// code part.
pub const RELEASE_READS: &[(&str, &str)] = &[
    (
        "base",
        "the descriptive name's base, and the BIDS facts (nils-release run.rs)",
    ),
    (
        "body_part",
        "the descriptive name's body part and spinal cord (nils-release run.rs)",
    ),
    (
        "construct",
        "the descriptive name and the BIDS facts (nils-release run.rs)",
    ),
    (
        "directory_type",
        "the descriptive layout's folder and the BIDS datatype (nils-release run.rs)",
    ),
    (
        "disposition",
        "what a release does with a stack: an acquisition, a derivative, a scout or a working scan (nils-release run.rs)",
    ),
    (
        "modifier",
        "the descriptive name and the BIDS facts (nils-release run.rs)",
    ),
    (
        "post_contrast",
        "the descriptive name's contrast and the BIDS ce- entity (nils-release run.rs, bids/name.rs)",
    ),
    (
        "provenance",
        "the descriptive layout's folder and the BIDS rec- entity (nils-release run.rs, bids/name.rs)",
    ),
    (
        "technique",
        "the descriptive name and the BIDS facts (nils-release run.rs)",
    ),
    (
        "acceleration",
        "the descriptive name's acceleration (nils-release run.rs)",
    ),
    (
        "task",
        "the BIDS task entity, a person's answer (nils-release run.rs)",
    ),
];

/// The axis a pick run finds its candidates by (nils-classify picking.rs):
/// a stack is a candidate for the roles it holds.
pub const PICK_CANDIDATES: &str = "role";

/// The axes of a pack that matter, each with why.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Matters {
    /// Axis name to the reasons it matters, in the order found.
    pub axes: BTreeMap<String, Vec<String>>,
}

impl Matters {
    pub fn contains(&self, axis: &str) -> bool {
        self.axes.contains_key(axis)
    }

    fn add(&mut self, pack: &Pack, axis: &str, why: String) {
        if pack.axes.iter().any(|a| a.name == axis) {
            let reasons = self.axes.entry(axis.to_string()).or_default();
            if !reasons.contains(&why) {
                reasons.push(why);
            }
        }
    }
}

/// The axes of `pack` that matter: those a release's name reads, those its
/// pick models read, and those a release needs.
pub fn of(pack: &Pack) -> Matters {
    let mut m = Matters::default();
    for (axis, why) in RELEASE_READS {
        m.add(pack, axis, format!("release: {why}"));
    }
    let b = &pack.bids;
    if !b.is_empty() {
        m.add(
            pack,
            "directory_type",
            "BIDS: the datatype of an intent (bids.yml datatypes)".to_string(),
        );
    }
    for (axis, map) in [
        ("construct", b.from_construct.len()),
        ("technique", b.from_technique.len()),
        ("modifier", b.from_modifier.len()),
        ("base", b.from_base.len()),
    ] {
        if map > 0 {
            m.add(
                pack,
                axis,
                format!("BIDS: a suffix from the {axis} (bids.yml suffix)"),
            );
        }
    }
    if !b.part.is_empty() {
        m.add(
            pack,
            "construct",
            "BIDS: the part entity (bids.yml)".to_string(),
        );
    }
    if !b.mtransfer.is_empty() {
        m.add(
            pack,
            "modifier",
            "BIDS: the mt entity (bids.yml)".to_string(),
        );
    }
    if !b.reconstruction.is_empty() {
        for axis in ["provenance", "construct"] {
            m.add(pack, axis, "BIDS: the rec entity (bids.yml)".to_string());
        }
    }
    if !b.ceagent.is_empty() {
        m.add(
            pack,
            "post_contrast",
            "BIDS: the ce entity (bids.yml ceagent)".to_string(),
        );
    }
    for t in &b.acq {
        m.add(
            pack,
            &t.from,
            format!("BIDS: the acq- group from {} (bids.yml acq)", t.from),
        );
    }
    if !b.synthetic_provenance.is_empty() {
        m.add(
            pack,
            "provenance",
            "BIDS: a synthetic contrast's place (bids.yml)".to_string(),
        );
    }
    if !b.synthetic_construct.is_empty() || !b.derivative_construct.is_empty() {
        m.add(
            pack,
            "construct",
            "BIDS: a synthetic contrast's or a derivative's place (bids.yml)".to_string(),
        );
    }
    for f in &b.folders {
        if !f.provenance.is_empty() {
            m.add(
                pack,
                "provenance",
                format!("BIDS: the {} folder (bids.yml folders)", f.name),
            );
        }
        if !f.technique.is_empty() {
            m.add(
                pack,
                "technique",
                format!("BIDS: the {} folder (bids.yml folders)", f.name),
            );
        }
    }
    for model in &pack.picks {
        m.add(
            pack,
            PICK_CANDIDATES,
            format!("pick {}: the roles it picks for", model.name),
        );
        for name in model.reads() {
            m.add(
                pack,
                &name,
                format!("pick {}: read to score and compare", model.name),
            );
        }
    }
    m
}

/// The axes where a stack with no answer raises `<axis>:missing`: those that
/// matter, hold one value, have no default and are not left to an image
/// model, in the pack's order.
pub fn missing_asked(pack: &Pack) -> Vec<String> {
    let m = of(pack);
    pack.axes
        .iter()
        .filter(|a| m.contains(&a.name))
        .filter(|a| !a.multi && a.default.is_none())
        .filter(|a| !pack.review.by_model.contains(&a.name))
        .map(|a| a.name.clone())
        .collect()
}
