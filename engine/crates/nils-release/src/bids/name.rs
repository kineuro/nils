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
    pub body_part: Option<&'a str>,
    pub provenance: Option<&'a str>,
    pub orientation: Option<&'a str>,
    pub acquisition_type: Option<&'a str>,
    pub post_contrast: bool,
    /// What the subject was doing, which only a person can say (§9.2).
    pub task: Option<&'a str>,
    /// The measured echo number, where the series has more than one.
    pub echo: Option<i64>,
    /// The phase encoding direction, as `AP`, `PA`, `LR` or `RL`.
    pub pe_direction: Option<&'a str>,
}

/// A name the standard admits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Name {
    pub datatype: &'static str,
    pub suffix: &'static str,
    /// Entity key to value, **in the schema's order**, which is the order a
    /// filename spells them.
    pub entities: Vec<(&'static str, String)>,
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
        }
    }
}

/// Build the name, or say why there is none.
pub fn build(facts: &Facts, map: &Mapping) -> Result<Name, Why> {
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

    let mut have: BTreeMap<&'static str, String> = BTreeMap::new();
    let mut set = |key: &str, value: String| {
        if let Some(e) = schema::entity(key)
            && group.allowed.contains(&e.key)
        {
            have.insert(e.key, value);
        }
        // An entity the group does not admit is dropped rather than written:
        // `part` on a scanner-derived ADC is not a BIDS name. Dropping can
        // make two stacks share a name, which is what `run` is for.
    };

    // What the suffix itself fixes. First, because an `INV1` is an MP2RAGE's
    // first inversion and saying so is part of saying what it is.
    for (key, value) in &named.entities {
        set(key, value.clone());
    }
    if let Some(task) = facts.task {
        set("task", task.to_string());
    }
    let acq = acq_label(facts, map);
    if !acq.is_empty() {
        set("acquisition", acq);
    }
    if facts.post_contrast && !map.ceagent.is_empty() {
        set("ceagent", map.ceagent.clone());
    }
    if let Some(dir) = facts.pe_direction {
        set("direction", dir.to_string());
    }
    if let Some(echo) = facts.echo.filter(|e| *e > 0) {
        set("echo", echo.to_string());
    }
    if let Some(mt) = map.mtransfer_of(&facts.modifiers) {
        set("mtransfer", mt.to_string());
    }
    if let Some(part) = map.part_of(&facts.constructs) {
        set("part", part.to_string());
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
    })
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

    /// Add `run-<n>`, which is the standard's answer to two acquisitions that
    /// are otherwise the same thing.
    ///
    /// It fails when the group does not admit `run`, which is how a caller
    /// learns that two stacks it cannot tell apart cannot be told apart in a
    /// BIDS name either.
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
fn acq_label(facts: &Facts, map: &Mapping) -> String {
    let mut out = String::new();
    for group in &map.acq {
        let values: Vec<&str> = match group.from.as_str() {
            "body_part" => facts.body_part.into_iter().collect(),
            "orientation" => facts.orientation.into_iter().collect(),
            "acquisition_type" => facts.acquisition_type.into_iter().collect(),
            "technique" => facts.technique.into_iter().collect(),
            "modifier" => facts.modifiers.clone(),
            "construct" => facts.constructs.clone(),
            "provenance" => facts.provenance.into_iter().collect(),
            // A field the engine does not supply contributes nothing rather
            // than silently becoming an empty token.
            _ => Vec::new(),
        };
        for v in values {
            if let Some(token) = group.tokens.get(v) {
                out.push_str(token);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
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
        m.ceagent = "contrast".into();
        m.acq = vec![
            Tokens {
                from: "body_part".into(),
                tokens: [("spine".to_string(), "Spine".to_string())].into(),
            },
            Tokens {
                from: "technique".into(),
                tokens: [
                    ("MPRAGE".to_string(), "MPRAGE".to_string()),
                    ("ME-GRE".to_string(), "MEGRE".to_string()),
                ]
                .into(),
            },
            Tokens {
                from: "modifier".into(),
                tokens: [("FatSat".to_string(), "FatSat".to_string())].into(),
            },
        ];
        m
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
        let n = build(&t1w(), &mapping()).unwrap();
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
            body_part: Some("spine"),
            modifiers: vec!["FatSat", "MT"],
            constructs: vec!["Magnitude"],
            post_contrast: true,
            echo: Some(2),
            ..Facts::default()
        };
        let n = build(&facts, &mapping()).unwrap();
        assert_eq!(
            n.stem("x", "1"),
            "sub-x_ses-1_acq-SpineMPRAGEFatSat_ce-contrast_echo-2_part-mag_T1w"
        );
        // And `mt-` is not there, though the stack says `MT`: the schema gives
        // `mt` only to `MTR`, `MTS` and `MPM`, each computed from more than one
        // image. That is the same fact as `MTw` having no BIDS name at all.
        assert!(!n.stem("x", "1").contains("mt-"));
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
            ..Facts::default()
        };
        assert_eq!(
            build(&facts, &mapping()),
            Err(Why::Missing("echo", "MEGRE".into()))
        );
        let with_echo = Facts {
            echo: Some(3),
            ..facts
        };
        assert_eq!(
            build(&with_echo, &mapping()).unwrap().stem("x", "1"),
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
        assert_eq!(build(&facts, &mapping()), Err(Why::NoTask));
        let answered = Facts {
            task: Some("rest"),
            ..facts
        };
        assert_eq!(
            build(&answered, &mapping()).unwrap().stem("x", "1"),
            "sub-x_ses-1_task-rest_bold"
        );
    }

    #[test]
    fn an_entity_the_group_does_not_admit_is_dropped_rather_than_written() {
        // The schema gives `dwi`'s scanner derivatives no `part`, so a
        // magnitude ADC map is an ADC map and not an invalid name. Two stacks
        // that lose what told them apart are what `run` is for.
        let facts = Facts {
            intent: Some("dwi"),
            constructs: vec!["ADC", "Magnitude"],
            base: Some("DWI"),
            ..Facts::default()
        };
        let n = build(&facts, &mapping()).unwrap();
        assert_eq!(n.stem("x", "1"), "sub-x_ses-1_ADC");
        assert_eq!(n.datatype, "dwi");
    }

    #[test]
    fn a_direction_reaches_a_dwi_and_not_an_anat() {
        // `dir` is optional on `dwi` and absent from every `anat` group, so
        // the same fact lands in one name and not the other.
        let dwi = Facts {
            intent: Some("dwi"),
            base: Some("DWI"),
            pe_direction: Some("AP"),
            ..Facts::default()
        };
        assert_eq!(
            build(&dwi, &mapping()).unwrap().stem("x", "1"),
            "sub-x_ses-1_dir-AP_dwi"
        );
        let anat = Facts {
            pe_direction: Some("AP"),
            ..t1w()
        };
        assert_eq!(
            build(&anat, &mapping()).unwrap().stem("x", "1"),
            "sub-x_ses-1_T1w"
        );
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
            build(&facts, &mapping()).unwrap().stem("x", "1"),
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
        assert_eq!(build(&swi, &mapping()), Err(Why::NoSuffix));
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
        assert!(build(&scout, &mapping()).is_ok());
        assert_eq!(
            build(&nothing, &mapping()),
            Err(Why::NoDatatype("localizer".into()))
        );
    }

    #[test]
    fn run_is_added_only_where_the_standard_admits_it() {
        let n = build(&t1w(), &mapping()).unwrap();
        assert_eq!(
            n.with_run(2).unwrap().stem("x", "1"),
            "sub-x_ses-1_run-2_T1w"
        );
        // And it stays in the standard's order, before echo and part.
        let facts = Facts {
            echo: Some(1),
            ..t1w()
        };
        let n = build(&facts, &mapping()).unwrap().with_run(3).unwrap();
        assert_eq!(n.stem("x", "1"), "sub-x_ses-1_run-3_echo-1_T1w");
    }

    #[test]
    fn a_session_that_is_not_named_leaves_the_entity_out() {
        let n = build(&t1w(), &mapping()).unwrap();
        assert_eq!(n.stem("x", ""), "sub-x_T1w");
        assert_eq!(n.dir("x", ""), "sub-x/anat");
    }
}
