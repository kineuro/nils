// SPDX-License-Identifier: AGPL-3.0-only

//! A BIDS name, built from the decided axes and checked against the schema
//! (`docs/specs/wave3-anonymize-and-bids.md`, §9.2).
//!
//! The grammar, in the standard's order:
//!
//! ```text
//! sub-<label>[_ses-<label>][_task-<label>][_acq-<label>][_ce-<label>]
//! [_rec-<label>][_dir-<label>][_run-<index>][_echo-<index>][_flip-<index>]
//! [_inv-<index>][_mt-<label>][_part-<label>]_<suffix>.<ext>
//! ```
//!
//! Two halves, and which is which decides who may be wrong about what. The
//! **pack** says which of our values means `T1w`; the **schema** says what a
//! `T1w` file may be called. So this module never guesses: a stack it cannot
//! name is refused with the reason, and §9.3 routes it. **A name that is not
//! valid is not written**, because the whole value of the layout is that a
//! validator passes it.
//!
//! v0 writes no entity name at all: its "BIDS export" puts the descriptive
//! name of §9.1 in a folder named by the intent. So nothing here is carried
//! from it, and its four naming bugs are not reproducible here because the
//! entity grammar is what removes the need for a counter.

use std::collections::BTreeMap;

use nils_pack::bids::Mapping;

use super::schema;

/// What a stack is, in the words the pack and the fingerprint use.
///
/// Every axis value is an **identity** and not what a row stores: `base`
/// stores `T2*w` and its identity is `T2starw`, which is also the word BIDS
/// uses. The caller reads a row through the axis to get here.
#[derive(Debug, Clone, Default)]
pub struct Facts<'a> {
    /// The pack's intent, which the datatype comes from unless a suffix
    /// overrides it.
    pub intent: Option<&'a str>,
    pub constructs: Vec<&'a str>,
    pub technique: Option<&'a str>,
    pub modifiers: Vec<&'a str>,
    pub base: Option<&'a str>,
    pub provenance: Option<&'a str>,
    pub post_contrast: bool,
    /// What the subject was doing, which only a person can say (§9.2).
    pub task: Option<&'a str>,
    /// The measured echo number, where the series has more than one.
    pub echo: Option<i64>,
    /// The phase encoding direction, as `AP`, `PA`, `LR` or `RL`.
    pub pe_direction: Option<&'a str>,
    /// Everything the stack says, by the name the pack knows it under: the
    /// axis or the field, to its values as identities.
    ///
    /// The whole of it, and not a list this module was written knowing.
    /// `acq-` carries what no entity takes, the pack declares what that is,
    /// and a pack that gives itself an axis can put it in a name without the
    /// engine learning the axis first. Before record 37 S6 the list lived
    /// here, and the quality axis, which says what a file claims is wrong
    /// with its own image, could not reach a filename at all.
    pub axes: BTreeMap<&'a str, Vec<&'a str>>,
}

/// A name the standard admits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Name {
    pub datatype: &'static str,
    pub suffix: &'static str,
    /// Entity key to value, **in the schema's order**, which is the order a
    /// filename spells them.
    pub entities: Vec<(&'static str, String)>,
    /// The entities the schema refuses this suffix, whose facts went into
    /// `acq-` instead (§9.2, record 37 S6). Never dropped and never silent:
    /// the run counts them by entity, so a tree says how often the standard
    /// had no slot for something the archive states.
    pub refused: Vec<&'static str>,
}

/// Why a stack has no BIDS name.
///
/// Never a silent drop: every one of these is reported per subject and
/// session, and §9.3 decides where the stack goes instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Why {
    /// The pack's intent has no BIDS datatype. A scout is the case: BIDS has
    /// no datatype for one.
    NoDatatype(String),
    /// Nothing the stack says names a suffix. An SWI image is the case: BIDS
    /// has no word for one, and inventing one puts a claim in a filename.
    NoSuffix,
    /// The pack named a suffix the standard does not have in that datatype.
    /// A load-time check cannot catch this, because the datatype may come
    /// from the intent rather than from the mapping.
    NotInSchema(String, String),
    /// The suffix requires an entity the stack cannot supply. `MEGRE` requires
    /// `echo`, and a multi-echo series whose echo numbers were never recorded
    /// is exactly v0's second export bug, caught here as an error instead of
    /// written out as a name.
    Missing(&'static str, String),
    /// `func` requires `task`, and no rule can invent one (§9.2).
    NoTask,
    /// Record 37 S2. Two or more stacks of one subject, session and datatype
    /// built this same name, and they are not one acquisition done twice, so
    /// there is no honest name here: `run-` would claim a rescan that never
    /// happened, and a counter of ours would claim nothing at all while
    /// looking exactly like one. `differs` is what
    /// [`super::repeat::one_acquisition`] found between them, which is also
    /// the evidence a pack extension would need.
    Shared { others: usize, differs: String },
}

impl std::fmt::Display for Why {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Why::NoDatatype(i) => write!(f, "BIDS has no datatype for {i}"),
            Why::NoSuffix => f.write_str("nothing it says names a BIDS suffix"),
            Why::NotInSchema(d, s) => write!(
                f,
                "{s} is not a {d} suffix in BIDS {}",
                schema::BIDS_VERSION
            ),
            Why::Missing(e, s) => write!(f, "{s} requires {e} and the stack does not say"),
            Why::NoTask => {
                f.write_str("func requires task and nobody has said what the subject was doing")
            }
            Why::Shared { others, differs } => write!(
                f,
                "it would share one BIDS name with {others} other stack(s) of this session: \
                 {differs}"
            ),
        }
    }
}

impl Why {
    /// A short word for a report's tally, so a run can count its reasons
    /// without repeating a sentence per stack.
    pub fn kind(&self) -> &'static str {
        match self {
            Why::NoDatatype(_) => "no_datatype",
            Why::NoSuffix => "no_suffix",
            Why::NotInSchema(_, _) => "not_in_schema",
            Why::Missing(_, _) => "missing_entity",
            Why::NoTask => "no_task",
            Why::Shared { .. } => "shared_name",
        }
    }
}

/// An entity a stack would take, its value, and the axis values it was read
/// from, which is what a refusal needs to know whether `acq-` says it already.
type Wanted<'a> = (&'a str, String, Vec<(&'a str, &'a str)>);

/// Build the name, or say why there is none.
///
/// Every fact the stack states reaches the name or is said to have been
/// refused. An entity the schema does not give this suffix is **not dropped**,
/// which is what this did before record 37 S6: its fact is spelled into
/// `acq-`, which is free-form, unless the label carries it already, and either
/// way the entity is named in `refused` so the run can count it.
pub fn build(facts: &Facts, map: &Mapping, naming: crate::name::Naming) -> Result<Name, Why> {
    let named = map.suffix(
        &facts.constructs,
        facts.technique,
        &facts.modifiers,
        facts.base,
    );
    // The datatype comes from the suffix where the mapping fixes one, because
    // a suffix is the stronger statement: an ADC map is `dwi` whatever folder
    // the intent cascade put it in.
    let datatype = match named {
        Some(n) => n.datatype.clone(),
        None => match facts.intent.and_then(|i| map.datatypes.get(i)) {
            Some(d) => d.clone(),
            None => {
                return Err(Why::NoDatatype(
                    facts.intent.unwrap_or("nothing").to_string(),
                ));
            }
        },
    };
    let named = named.ok_or(Why::NoSuffix)?;
    let group = schema::group_of(&datatype, &named.suffix)
        .ok_or_else(|| Why::NotInSchema(datatype.clone(), named.suffix.clone()))?;

    // What the stack would say in an entity, each with the axis value it came
    // from, which is how a refusal knows whether `acq-` says it already.
    let mut want: Vec<Wanted> = Vec::new();
    // What the suffix itself fixes. First, because an `INV1` is an MP2RAGE's
    // first inversion and saying so is part of saying what it is.
    for (key, value) in &named.entities {
        want.push((key.as_str(), value.clone(), Vec::new()));
    }
    if let Some(task) = facts.task {
        want.push(("task", task.to_string(), Vec::new()));
    }
    if facts.post_contrast && !map.ceagent.is_empty() {
        want.push((
            "ceagent",
            map.ceagent.clone(),
            vec![("post_contrast", "given")],
        ));
    }
    // §9.2 with record 37 S6: what made the image out of the acquisition. The
    // scanner's pipeline and the reformats and projections both, because both
    // are reconstructions and `rec-` is where the standard puts them.
    if let Some((from, label)) = map.reconstruction_of(facts.provenance, &facts.constructs) {
        let source = from
            .iter()
            .map(|v| match facts.provenance == Some(*v) {
                true => ("provenance", *v),
                false => ("construct", *v),
            })
            .collect();
        want.push(("reconstruction", label, source));
    }
    if let Some(dir) = facts.pe_direction {
        want.push(("direction", dir.to_string(), Vec::new()));
    }
    if let Some(echo) = facts.echo.filter(|e| *e > 0) {
        want.push(("echo", echo.to_string(), Vec::new()));
    }
    if let Some((from, mt)) = map.mtransfer_of(&facts.modifiers) {
        want.push(("mtransfer", mt.to_string(), vec![("modifier", from)]));
    }
    if let Some((from, part)) = map.part_of(&facts.constructs) {
        want.push(("part", part.to_string(), vec![("construct", from)]));
    }

    let mut have: BTreeMap<&'static str, String> = BTreeMap::new();
    let mut refused: Vec<&'static str> = Vec::new();
    // The tokens a refused entity contributes to `acq-`, with the entity's
    // position in the grammar, so that they are joined in the standard's own
    // order rather than in the order this function happened to ask.
    let mut spelled: Vec<(usize, String)> = Vec::new();
    for (key, value, from) in want {
        let Some(e) = schema::entity(key) else {
            continue;
        };
        if group.allowed.contains(&e.key) || group.required.contains(&e.key) {
            have.insert(e.key, value);
            continue;
        }
        // The schema gives this suffix no such entity: `ce` on a diffusion
        // image, `part` on a susceptibility map, `mt` on anything this pack
        // names. Dropping it, which is what happened before, loses a fact and
        // makes two stacks share a name; so it is spelled into `acq-`
        // instead, unless the label carries the same fact already.
        refused.push(e.key);
        if !from.is_empty()
            && from
                .iter()
                .all(|(axis, v)| map.acq_carries(naming.name(), axis, v))
        {
            continue;
        }
        let at = schema::ENTITIES.iter().position(|x| x.key == e.key);
        spelled.push((at.unwrap_or(usize::MAX), token(e.name, &value)));
    }
    spelled.sort();
    let mut acq = acq_label(facts, map, naming);
    for (_, token) in spelled {
        acq.push_str(&token);
    }
    if !acq.is_empty()
        && let Some(e) = schema::entity("acquisition")
        && group.allowed.contains(&e.key)
    {
        have.insert(e.key, acq);
    }

    // `func` requires `task`, and the reason is worth its own answer: the
    // fact is missing, not the name.
    if group.required.contains(&"task") && !have.contains_key("task") {
        return Err(Why::NoTask);
    }
    for key in group.required {
        if !have.contains_key(key) {
            return Err(Why::Missing(key, named.suffix.clone()));
        }
    }
    // In the schema's order, so a name is in the standard's order by
    // construction rather than by care.
    let entities: Vec<(&'static str, String)> = schema::ENTITIES
        .iter()
        .filter_map(|e| have.remove(e.key).map(|v| (e.key, v)))
        .filter(|(_, v)| !v.is_empty())
        .collect();
    for (key, value) in &entities {
        if !schema::admits(key, value) {
            return Err(Why::Missing(
                schema::entity(key).map(|e| e.key).unwrap_or(key),
                format!("{} with {key}-{value}", named.suffix),
            ));
        }
    }
    Ok(Name {
        datatype: schema::datatype(&datatype).ok_or(Why::NoSuffix)?,
        suffix: group
            .suffixes
            .iter()
            .find(|s| **s == named.suffix)
            .copied()
            .ok_or(Why::NoSuffix)?,
        entities,
        refused,
    })
}

/// A fact the schema refuses an entity for, as an `acq-` token.
///
/// The entity's own word and its value, each with a capital, so that a reader
/// of `acq-BrainAx2DDWIEPICeContrast` can see which entity the standard would
/// not take: `ce-contrast` refused reads `CeContrast`, `part-mag` reads
/// `PartMag`, `echo-2` reads `Echo2`. A BIDS label is `[0-9a-zA-Z+]+`, so
/// anything else is dropped from the token rather than spelled.
fn token(name: &str, value: &str) -> String {
    let capital = |text: &str| -> String {
        let mut chars = text.chars();
        match chars.next() {
            Some(c) => c.to_uppercase().chain(chars).collect(),
            None => String::new(),
        }
    };
    format!("{}{}", capital(name), capital(value))
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '+')
        .collect()
}

impl Name {
    /// The filename without its extension, which is the whole name a
    /// converter is given.
    pub fn stem(&self, subject: &str, session: &str) -> String {
        let mut out = format!("sub-{subject}");
        if !session.is_empty() {
            out.push_str(&format!("_ses-{session}"));
        }
        for (key, value) in &self.entities {
            let name = schema::entity(key).map(|e| e.name).unwrap_or(key);
            out.push_str(&format!("_{name}-{value}"));
        }
        out.push('_');
        out.push_str(self.suffix);
        out
    }

    /// Where in the tree it goes, under a subject and session.
    pub fn dir(&self, subject: &str, session: &str) -> String {
        match session.is_empty() {
            true => format!("sub-{subject}/{}", self.datatype),
            false => format!("sub-{subject}/ses-{session}/{}", self.datatype),
        }
    }

    /// Add `run-<n>`, which is the standard's word for one acquisition made
    /// again.
    ///
    /// Only the caller may decide that: record 37 S2 measures whether two
    /// stacks really are one acquisition twice
    /// ([`super::repeat::one_acquisition`]), and this writes the entity once
    /// that is settled. Two stacks that merely want one filename are not a
    /// repeat, and 67 per cent of the indices a counter wrote were that.
    ///
    /// It fails when the group does not admit `run`, which is how a caller
    /// learns that even a true repeat cannot be said in a BIDS name here, and
    /// that the second stack would otherwise rewrite the first.
    pub fn with_run(&self, n: i64) -> Option<Name> {
        let group = schema::group_of(self.datatype, self.suffix)?;
        if !group.allowed.contains(&"run") {
            return None;
        }
        let mut have: Vec<(&'static str, String)> = self
            .entities
            .iter()
            .filter(|(k, _)| *k != "run")
            .cloned()
            .collect();
        have.push(("run", n.to_string()));
        let mut out = self.clone();
        out.entities = schema::ENTITIES
            .iter()
            .filter_map(|e| {
                have.iter()
                    .find(|(k, _)| *k == e.key)
                    .map(|(k, v)| (*k, v.clone()))
            })
            .collect();
        Some(out)
    }
}

/// Everything that describes the acquisition and is not a suffix or an entity,
/// joined into one label in the pack's declared order.
///
/// **The pack says what a name carries.** This reads the axis the pack names
/// off what the stack says, whatever that axis is, so a pack that gives itself
/// one can put it in a name and the engine needs no release to learn it. An
/// axis the stack is silent on contributes nothing, and a value with no token
/// contributes nothing, which is how `ND` and `RawRecon` stay out of every
/// filename.
fn acq_label(facts: &Facts, map: &Mapping, naming: crate::name::Naming) -> String {
    let mut out = String::new();
    for group in map.acq.iter().filter(|g| g.in_mode(naming.name())) {
        for value in facts
            .axes
            .get(group.from.as_str())
            .into_iter()
            .flatten()
            .filter_map(|v| group.tokens.get(*v))
        {
            out.push_str(value);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::name::Naming;
    use nils_pack::bids::{Named, Tokens};

    fn named(datatype: &str, suffix: &str) -> Named {
        Named {
            datatype: datatype.into(),
            suffix: suffix.into(),
            when_technique: None,
            entities: BTreeMap::new(),
        }
    }

    fn mapping() -> Mapping {
        let mut m = Mapping::default();
        for i in ["anat", "dwi", "func", "fmap", "perf"] {
            m.datatypes.insert(i.into(), i.into());
        }
        m.from_base.insert("T1w".into(), named("anat", "T1w"));
        m.from_base.insert("T2w".into(), named("anat", "T2w"));
        m.from_base.insert("DWI".into(), named("dwi", "dwi"));
        m.from_construct.insert("ADC".into(), named("dwi", "ADC"));
        m.from_technique
            .insert("ME-GRE".into(), named("anat", "MEGRE"));
        m.from_technique
            .insert("BOLD-EPI".into(), named("func", "bold"));
        m.from_modifier
            .insert("FLAIR".into(), named("anat", "FLAIR"));
        let mut inv = named("anat", "MP2RAGE");
        inv.when_technique = Some("MP2RAGE".into());
        inv.entities.insert("inversion".into(), "1".into());
        m.from_construct.insert("INV1".into(), inv);
        m.part.insert("Magnitude".into(), "mag".into());
        m.part.insert("Phase".into(), "phase".into());
        m.mtransfer.insert("MT".into(), "on".into());
        m.reconstruction
            .insert("DTIRecon".into(), "DTIRecon".into());
        m.reconstruction.insert("MIP".into(), "MIP".into());
        m.ceagent = "contrast".into();
        m.acq = vec![
            group("body_part", &[], &[("spine", "Spine")]),
            group(
                "technique",
                &[],
                &[("MPRAGE", "MPRAGE"), ("ME-GRE", "MEGRE")],
            ),
            group("modifier", &[], &[("FatSat", "FatSat"), ("MT", "MT")]),
            group("quality", &[], &[("Distorted", "Distorted")]),
            // The informative name only: in a BIDS name these are `part-`
            // and `rec-`, and a name that said it twice would be a name
            // arguing with itself (record 37 S7).
            group(
                "construct",
                &["informative"],
                &[("Magnitude", "Mag"), ("MIP", "MIP")],
            ),
            group("provenance", &["informative"], &[("DTIRecon", "DTIRecon")]),
        ];
        m
    }

    /// One `acq-` group, as a pack declares it.
    fn group(from: &str, modes: &[&str], tokens: &[(&str, &str)]) -> Tokens {
        Tokens {
            from: from.to_string(),
            modes: modes.iter().map(|m| m.to_string()).collect(),
            tokens: tokens
                .iter()
                .map(|(v, t)| (v.to_string(), t.to_string()))
                .collect(),
        }
    }

    /// What a stack says, by axis, which is what `acq-` is built from.
    fn axes<'a>(said: &[(&'a str, &'a str)]) -> BTreeMap<&'a str, Vec<&'a str>> {
        let mut out: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (axis, value) in said {
            out.entry(axis).or_default().push(value);
        }
        out
    }

    fn t1w() -> Facts<'static> {
        Facts {
            intent: Some("anat"),
            base: Some("T1w"),
            ..Facts::default()
        }
    }

    #[test]
    fn the_simplest_name_is_the_subject_the_session_and_the_suffix() {
        let n = build(&t1w(), &mapping(), Naming::Bids).unwrap();
        assert_eq!(n.stem("x", "M06"), "sub-x_ses-M06_T1w");
        assert_eq!(n.dir("x", "M06"), "sub-x/ses-M06/anat");
    }

    #[test]
    fn the_entities_come_out_in_the_standards_order() {
        // Not in the order they were worked out, and not in alphabetical
        // order: the schema's, which is what a validator reads.
        let facts = Facts {
            intent: Some("anat"),
            base: Some("T1w"),
            technique: Some("MPRAGE"),
            modifiers: vec!["FatSat", "MT"],
            constructs: vec!["Magnitude"],
            post_contrast: true,
            echo: Some(2),
            axes: axes(&[
                ("body_part", "spine"),
                ("technique", "MPRAGE"),
                ("modifier", "FatSat"),
                ("modifier", "MT"),
                ("construct", "Magnitude"),
            ]),
            ..Facts::default()
        };
        let n = build(&facts, &mapping(), Naming::Bids).unwrap();
        assert_eq!(
            n.stem("x", "1"),
            "sub-x_ses-1_acq-SpineMPRAGEFatSatMT_ce-contrast_echo-2_part-mag_T1w"
        );
        // And `mt-` is not there, though the stack says `MT`: the schema gives
        // `mt` only to `MTR`, `MTS` and `MPM`, each computed from more than one
        // image. That is the same fact as `MTw` having no BIDS name at all.
        // The fact is not lost with it: the pack puts `MT` in `acq-`, the name
        // says so, and the refusal is counted rather than passed over.
        assert!(!n.stem("x", "1").contains("mt-"));
        assert!(n.stem("x", "1").contains("acq-SpineMPRAGEFatSatMT"));
        // And the `MT` in the label is the pack's own token and not a second
        // spelling of the refusal: a fact already in the name is not said
        // twice to say it was refused.
        assert!(!n.stem("x", "1").contains("MtOn"));
        assert_eq!(n.refused, vec!["mtransfer"]);
    }

    #[test]
    fn a_suffix_that_requires_an_entity_is_not_written_without_it() {
        // v0's second export bug, as a refusal rather than a filename. `MEGRE`
        // requires `echo`, and a stack whose echo number is unknown has no
        // MEGRE name.
        let facts = Facts {
            intent: Some("anat"),
            technique: Some("ME-GRE"),
            base: Some("T2starw"),
            axes: axes(&[("technique", "ME-GRE")]),
            ..Facts::default()
        };
        assert_eq!(
            build(&facts, &mapping(), Naming::Bids),
            Err(Why::Missing("echo", "MEGRE".into()))
        );
        let with_echo = Facts {
            echo: Some(3),
            ..facts
        };
        assert_eq!(
            build(&with_echo, &mapping(), Naming::Bids)
                .unwrap()
                .stem("x", "1"),
            "sub-x_ses-1_acq-MEGRE_echo-3_MEGRE"
        );
    }

    #[test]
    fn func_without_a_task_says_the_fact_is_missing_and_not_the_name() {
        let facts = Facts {
            intent: Some("func"),
            technique: Some("BOLD-EPI"),
            ..Facts::default()
        };
        assert_eq!(build(&facts, &mapping(), Naming::Bids), Err(Why::NoTask));
        let answered = Facts {
            task: Some("rest"),
            ..facts
        };
        assert_eq!(
            build(&answered, &mapping(), Naming::Bids)
                .unwrap()
                .stem("x", "1"),
            "sub-x_ses-1_task-rest_bold"
        );
    }

    #[test]
    fn an_entity_the_group_does_not_admit_is_spelled_and_not_dropped() {
        // The schema gives `dwi`'s scanner derivatives no `part`, so a
        // magnitude ADC map cannot say `part-mag` and be a BIDS name. It used
        // to drop the fact, which makes two stacks share a name and calls the
        // second a repeat of the first; record 37 S6 spells it into `acq-`,
        // which is free-form, and names the entity that was refused.
        let facts = Facts {
            intent: Some("dwi"),
            constructs: vec!["ADC", "Magnitude"],
            base: Some("DWI"),
            axes: axes(&[("construct", "ADC"), ("construct", "Magnitude")]),
            ..Facts::default()
        };
        let n = build(&facts, &mapping(), Naming::Bids).unwrap();
        assert_eq!(n.stem("x", "1"), "sub-x_ses-1_acq-PartMag_ADC");
        assert_eq!(n.datatype, "dwi");
        assert_eq!(n.refused, vec!["part"]);
    }

    #[test]
    fn a_direction_reaches_a_dwi_as_an_entity_and_an_anat_as_a_token() {
        // `dir` is optional on `dwi` and absent from every `anat` group, so
        // the same fact lands in one name as the entity the standard has for
        // it and in the other as an `acq-` token, which is the only place
        // left that can hold it (record 37 S6). Not in neither, which is
        // what dropping it used to mean.
        let dwi = Facts {
            intent: Some("dwi"),
            base: Some("DWI"),
            pe_direction: Some("AP"),
            ..Facts::default()
        };
        assert_eq!(
            build(&dwi, &mapping(), Naming::Bids)
                .unwrap()
                .stem("x", "1"),
            "sub-x_ses-1_dir-AP_dwi"
        );
        let anat = Facts {
            pe_direction: Some("AP"),
            ..t1w()
        };
        let n = build(&anat, &mapping(), Naming::Bids).unwrap();
        assert_eq!(n.stem("x", "1"), "sub-x_ses-1_acq-DirAP_T1w");
        assert_eq!(n.refused, vec!["direction"]);
    }

    #[test]
    fn a_suffix_brings_the_entity_that_is_part_of_saying_what_it_is() {
        let facts = Facts {
            intent: Some("anat"),
            constructs: vec!["INV1"],
            technique: Some("MP2RAGE"),
            base: Some("T1w"),
            ..Facts::default()
        };
        assert_eq!(
            build(&facts, &mapping(), Naming::Bids)
                .unwrap()
                .stem("x", "1"),
            "sub-x_ses-1_inv-1_MP2RAGE"
        );
    }

    #[test]
    fn a_stack_with_no_bids_word_is_refused_with_the_reason() {
        // BIDS has no suffix for an SWI image, and no datatype for a scout.
        // Neither is a failure: §9.3 routes them and the run reports them.
        let swi = Facts {
            intent: Some("anat"),
            base: Some("SWI"),
            ..Facts::default()
        };
        assert_eq!(build(&swi, &mapping(), Naming::Bids), Err(Why::NoSuffix));
        let scout = Facts {
            intent: Some("localizer"),
            base: Some("T1w"),
            ..Facts::default()
        };
        // The suffix decides the datatype where the mapping fixes one, so a
        // scout that reads as a T1w is named; one that says nothing is not.
        let nothing = Facts {
            intent: Some("localizer"),
            ..Facts::default()
        };
        assert!(build(&scout, &mapping(), Naming::Bids).is_ok());
        assert_eq!(
            build(&nothing, &mapping(), Naming::Bids),
            Err(Why::NoDatatype("localizer".into()))
        );
    }

    #[test]
    fn run_is_added_only_where_the_standard_admits_it() {
        let n = build(&t1w(), &mapping(), Naming::Bids).unwrap();
        assert_eq!(
            n.with_run(2).unwrap().stem("x", "1"),
            "sub-x_ses-1_run-2_T1w"
        );
        // And it stays in the standard's order, before echo and part.
        let facts = Facts {
            echo: Some(1),
            ..t1w()
        };
        let n = build(&facts, &mapping(), Naming::Bids)
            .unwrap()
            .with_run(3)
            .unwrap();
        assert_eq!(n.stem("x", "1"), "sub-x_ses-1_run-3_echo-1_T1w");
    }

    #[test]
    fn a_reconstruction_leaves_acq_for_the_entity_the_standard_has() {
        // Record 37 S6. The scanner's pipeline and the reformats are
        // reconstructions of an acquisition, `rec-` is allowed on every suffix
        // this pack writes, and moving them there separated no pair less over
        // 836 stacks while taking the longest tail off the label.
        let facts = Facts {
            intent: Some("anat"),
            base: Some("T1w"),
            technique: Some("MPRAGE"),
            provenance: Some("DTIRecon"),
            constructs: vec!["MIP"],
            axes: axes(&[
                ("technique", "MPRAGE"),
                ("provenance", "DTIRecon"),
                ("construct", "MIP"),
            ]),
            ..Facts::default()
        };
        let n = build(&facts, &mapping(), Naming::Bids).unwrap();
        assert_eq!(
            n.stem("x", "1"),
            "sub-x_ses-1_acq-MPRAGE_rec-DTIReconMIP_T1w"
        );
        assert!(n.refused.is_empty());
    }

    #[test]
    fn an_axis_the_engine_never_heard_of_reaches_a_name_because_the_pack_says_so() {
        // Record 37 S6, and the reason the list is not here any more: S5 gave
        // the pack a quality axis, and no release could write it because the
        // axes `acq-` may read were spelled into this module. Two stacks that
        // agree on everything and disagree on whether the image is whole are
        // not interchangeable, and now the name says which is which.
        let facts = Facts {
            intent: Some("anat"),
            base: Some("T1w"),
            technique: Some("MPRAGE"),
            axes: axes(&[("technique", "MPRAGE"), ("quality", "Distorted")]),
            ..Facts::default()
        };
        assert_eq!(
            build(&facts, &mapping(), Naming::Bids)
                .unwrap()
                .stem("x", "1"),
            "sub-x_ses-1_acq-MPRAGEDistorted_T1w"
        );
    }

    #[test]
    fn the_informative_name_carries_every_axis_and_the_bids_one_the_entities() {
        // Record 37 S7: one question asked of every name. In BIDS mode the
        // complex part is `part-` and the pipeline is `rec-`; in informative
        // mode the pack puts both in `acq-` as well, because a tree read by
        // people has no entities to look in.
        let facts = Facts {
            intent: Some("anat"),
            base: Some("T1w"),
            technique: Some("MPRAGE"),
            provenance: Some("DTIRecon"),
            constructs: vec!["Magnitude"],
            axes: axes(&[
                ("technique", "MPRAGE"),
                ("construct", "Magnitude"),
                ("provenance", "DTIRecon"),
            ]),
            ..Facts::default()
        };
        assert_eq!(
            build(&facts, &mapping(), Naming::Bids)
                .unwrap()
                .stem("x", "1"),
            "sub-x_ses-1_acq-MPRAGE_rec-DTIRecon_part-mag_T1w"
        );
        assert_eq!(
            build(&facts, &mapping(), Naming::Informative)
                .unwrap()
                .stem("x", "1"),
            "sub-x_ses-1_acq-MPRAGEMagDTIRecon_rec-DTIRecon_part-mag_T1w"
        );
    }

    #[test]
    fn a_session_that_is_not_named_leaves_the_entity_out() {
        let n = build(&t1w(), &mapping(), Naming::Bids).unwrap();
        assert_eq!(n.stem("x", ""), "sub-x_T1w");
        assert_eq!(n.dir("x", ""), "sub-x/anat");
    }
}
