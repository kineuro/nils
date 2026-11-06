// SPDX-License-Identifier: AGPL-3.0-only

//! Record 56, section 2 (2026-10-09): body part and post-contrast are their
//! own operations, each a step of a dataset and of a cohort with its own
//! review of what it is unsure of, and sorting asks nothing about either.
//! What the Data page reads of them:
//!
//! - **each operation's step** over a dataset's scans or a cohort's:
//!   whether it is served (a pipeline in the catalog proposes its axes and
//!   an admitted or promoted model answers them), the run over those scans
//!   that is queued, runs now or ran last, how many of them its model
//!   answered at or above its threshold, how many wait on a person, and why
//!   a run of it would be refused now;
//! - **what the sort asks**, which is all a card's certainty line counts,
//!   never one about an axis an operation owns: the one definition of a
//!   look, and who asks every other question, is `certainty`'s.
//!
//! A model runs over a frozen selection or a handle and names its models
//! (`nils run <pipeline> --select selection:<name>@<v> --model ...`). Since
//! 2026-10-10 a door starts an operation's run over a dataset's or a
//! cohort's scans: `POST /api/datasets/{name}/steps/{step}/run` and its
//! cohort twin freeze the scans the step counts into a handle and queue
//! `run <pipeline> --handle <id> --model ...` for them, a job of kind
//! `pipeline` that names its step and what it is for (`step`, `for`), which
//! the pipeline lane runs, so a run of hours never holds up a digest. The
//! step follows the job and then its run: queued, running, then done or
//! failed. Body part is the certified model of record 50, the pipeline
//! `bodypart-infer-fusion` given its registered parts (the encoder the head
//! names, the head that answers `body_part`, and the coarse mode that
//! answers `body_region` from the same encoder where one is served).
//! Post-contrast (record 55 E3) is the certified model r1 served through
//! its cascade, `postcontrast-cascade` given the promoted cascade's parts
//! (the few-slice student and r1's fusion): it answers on T1-type brain
//! and says "not sure" elsewhere, and what it defers goes to the full
//! pipeline, `postcontrast-full`, queued over those stacks when it is
//! installed. Its own label (record 55 F2 and F5, the time rule) is
//! `crate::contrast`'s, and its counts are on its step.
//!
//! Since 2026-11-05 a run may go over a step's "N without" alone: the door
//! takes `{"scans": "without"}` and freezes only the scans its hint counts
//! ([`without_condition`]), and each step says what a run would go over
//! before it starts (`runs`: every scan, and those without its answer, each
//! with the parts the run takes). `nils step run` queues the same run at
//! the command line ([`queue`]).
//!
//! Since 2026-11-06 a step runs over a saved selection as well, without a
//! cohort: `POST /api/selections/{name}/steps/{step}/run` and `nils step
//! run selection:NAME[@V]` freeze the selection whole, as `nils run
//! --select` freezes it, and the run goes the one way a dataset's does, over
//! a handle of those stacks ([`Scope::Handle`]); `GET
//! /api/selections/{name}/steps` says what each step's run over it would go
//! over before it starts. The sort's "N not sorted" is a run of its own too:
//! `{"scans": "without"}` on the sorted step sorts the scans no sort has
//! judged, those outside the rules left out ([`unsorted_condition`]).

use std::collections::BTreeSet;

use nils_registry::Registry;
use nils_registry::model::{self, Model};
use nils_registry::store::{Cell, Error as StoreError, Row, Store};
use serde_json::{Value, json};

use crate::serve::{Caller, Doors, Reply};

/// The operations that are steps of their own, by the step's name, with
/// the axes their models answer: the body part's fine and coarse modes
/// (record 50), and post-contrast.
pub(crate) const OPERATIONS: [(&str, &[&str]); 2] = [
    ("body_part", &["body_part", "body_region"]),
    ("post_contrast", &["post_contrast"]),
];

/// The pipeline body part's step runs: the certified model of record 50,
/// one frozen model with two modes, given its registered parts.
pub(crate) const BODY_PART_PIPELINE: &str = "bodypart-infer-fusion";

/// The pipeline post-contrast's step runs (record 55 E3): the certified
/// model r1 through its cascade, the few-slice student first.
pub(crate) const POST_CONTRAST_PIPELINE: &str = "postcontrast-cascade";

/// The full pipeline that answers what the cascade defers (record 55 E3,
/// spec §12.2), queued over the deferred stacks when it is installed.
pub(crate) const POST_CONTRAST_FULL: &str = "postcontrast-full";

/// How many of an operation's newest runs over the scans are looked
/// through for the one that runs and the jobs of its log.
const RUNS: usize = 50;

/// How many of the newest pipeline jobs are looked through for the ones a
/// door queued for a step.
const ASKED: usize = 200;

/// The operation of its own an axis belongs to, by its step's name; none
/// for an axis the sort answers.
pub(crate) fn owner(axis: &str) -> Option<&'static str> {
    OPERATIONS
        .iter()
        .find(|(_, axes)| axes.contains(&axis))
        .map(|(name, _)| *name)
}

/// An operation's step as a person reads it, in a sentence.
fn title(step: &str) -> &str {
    match step {
        "body_part" => "body part",
        "post_contrast" => "post-contrast",
        other => other,
    }
}

/// The scans a step counts.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Scope<'a> {
    /// A dataset's: the stacks its sources (a comma list of `source` ids)
    /// hold a file of, whoever read them first (record 55, 2026-10-10);
    /// none where the list is empty.
    Sources(&'a str),
    /// A cohort's: the stacks of its open members.
    Cohort(i64),
    /// A selection's (2026-11-06): the stacks a handle frozen from it now
    /// holds.
    Handle(i64),
}

/// Record 55 (Nima's duplicate policy, 2026-10-10): the condition a stack
/// aliased `alias` meets when one of the sources (a comma list of `source`
/// ids) holds a file of it, its instance's own or a copy, whoever read the
/// stack first. Every door that counts or lists a dataset's scans asks it,
/// so a dataset holds what its tree holds.
pub(crate) fn held_by(store: &Store, alias: &str, sources: &str) -> String {
    if sources.trim().is_empty() {
        return "1 = 0".to_string();
    }
    format!(
        "{alias}.id IN (SELECT ss.stack_id FROM {} ss WHERE ss.source_id IN ({sources}))",
        store.qualified("source_stack")
    )
}

/// The condition a study aliased `alias` meets when one of the sources
/// holds a file of a stack of it ([`held_by`]).
pub(crate) fn studies_held_by(store: &Store, alias: &str, sources: &str) -> String {
    if sources.trim().is_empty() {
        return "1 = 0".to_string();
    }
    format!(
        "{alias}.id IN (SELECT hse.study_id FROM {} hse JOIN {} hst ON hst.series_id = hse.id \
         JOIN {} hss ON hss.stack_id = hst.id WHERE hss.source_id IN ({sources}))",
        store.qualified("series"),
        store.qualified("stack"),
        store.qualified("source_stack")
    )
}

impl Scope<'_> {
    /// The condition a stack aliased `x` meets to be in scope.
    pub(crate) fn holds(&self, store: &Store) -> String {
        match self {
            Scope::Sources(ids) => held_by(store, "x", ids),
            Scope::Cohort(id) => format!(
                "x.series_id IN (SELECT se.id FROM {} se JOIN {} m ON m.subject_id = se.subject_id \
                 WHERE m.cohort_id = {id} AND m.left_at IS NULL)",
                store.qualified("series"),
                store.qualified("cohort_member"),
            ),
            Scope::Handle(id) => format!(
                "x.id IN (SELECT hm.key FROM {} hm WHERE hm.handle_id = {id})",
                store.qualified("handle_member")
            ),
        }
    }
}

/// A scope's condition with a sample sealed now left out where the caller
/// does not read sealed stacks (record 48): a step counts neither those
/// scans nor its model's answers on them for such a caller (the review of
/// Wave 7a's merge, 2026-10-10). A freeze for a run keeps them, and never
/// asks this.
fn shown(store: &Store, holds: String, hide_sealed: bool) -> String {
    if hide_sealed {
        format!(
            "({holds}) AND NOT EXISTS (SELECT 1 FROM {} sst WHERE sst.stack_id = x.id AND sst.unsealed_at IS NULL)",
            store.qualified("sealed_stack")
        )
    } else {
        holds
    }
}

fn list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn quoted(words: &[String]) -> String {
    words
        .iter()
        .map(|w| format!("'{}'", w.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ")
}

fn count(store: &mut Store, sql: &str) -> Result<i64, StoreError> {
    store.query(sql, &[])?[0].int(0)
}

/// The scans of a step's own "N without" (ruling 35 (b) of 2026-10-16, a
/// run over them alone since 2026-11-05): the stacks aliased `x` its model
/// would add an answer to, as a condition. For body part the scans with no
/// body part (no rule's and no model's answer); for post-contrast the scans
/// that can be T1-type brain by their stored axes (the test the cascade
/// applies, [`crate::score::can_be_t1_brain`]) whose contrast neither the
/// rules nor its model told. Its model's own answers, staged or put in
/// force since, count as told. Its hint counts these scans, and a run of
/// the step over `without` freezes these and no other.
fn without_condition(store: &Store, step: &str) -> String {
    let (axis, member, item) = (
        store.qualified("classification_axis"),
        store.qualified("review_member"),
        store.qualified("review_item"),
    );
    let groups: Vec<String> = OPERATIONS
        .iter()
        .find(|(name, _)| *name == step)
        .map(|(_, axes)| axes.iter().map(|a| format!("{a}:model")).collect())
        .unwrap_or_default();
    // what its model answered already, staged or put in force since
    let answered = format!(
        "EXISTS (SELECT 1 FROM {member} rm JOIN {item} ri ON ri.id = rm.item_id \
         WHERE rm.stack_id = x.id AND ri.kind IN ({}) AND ri.status IN ('staged', 'accepted'))",
        quoted(&groups)
    );
    if step == "post_contrast" {
        // Wave 7b: one grouped pass over the axes of the stacks whose base
        // is T1w, rather than eight lookups a stack over every stack in
        // scope (on 450,000 synthetic stacks under PostgreSQL, 3.7 s against
        // 20 s, the same count). The test is the one `can_be_t1_brain`
        // applies: the greatest base T1w and technique neither TOF nor PC;
        // neither body axis answered, or a body part of the brain, or a
        // region of the head; and no post-contrast answer.
        format!(
            "x.id IN (SELECT t.stack_id FROM (\
               SELECT a.stack_id, \
                 MAX(CASE WHEN a.axis = 'base' THEN a.value END) AS base, \
                 MAX(CASE WHEN a.axis = 'technique' THEN a.value END) AS technique, \
                 MAX(CASE WHEN a.axis = 'body_part' THEN a.value END) AS body_part, \
                 MAX(CASE WHEN a.axis = 'body_region' THEN 1 ELSE 0 END) AS region, \
                 MAX(CASE WHEN a.axis = 'body_region' AND a.value = 'head' THEN 1 ELSE 0 END) AS head, \
                 MAX(CASE WHEN a.axis = 'post_contrast' THEN 1 ELSE 0 END) AS told \
               FROM {axis} a \
               WHERE a.axis IN ('base', 'technique', 'body_part', 'body_region', 'post_contrast') \
                 AND a.value IS NOT NULL AND a.value <> '' \
                 AND a.stack_id IN (SELECT b.stack_id FROM {axis} b WHERE b.axis = 'base' AND b.value = 'T1w') \
               GROUP BY a.stack_id\
             ) t WHERE t.base = 'T1w' AND COALESCE(t.technique, '') NOT IN ('TOF', 'PC') \
               AND ((t.body_part IS NULL AND t.region = 0) OR t.body_part LIKE '%brain%' OR t.head = 1) \
               AND t.told = 0) AND NOT {answered}"
        )
    } else {
        format!(
            "NOT EXISTS (SELECT 1 FROM {axis} a WHERE a.stack_id = x.id AND a.axis = 'body_part' \
             AND a.value IS NOT NULL AND a.value <> '') AND NOT {answered}"
        )
    }
}

/// What a step's hint calls its scans without (ruling 35 (b)): the kind of
/// its count.
fn without_kind(step: &str) -> &'static str {
    if step == "post_contrast" {
        "t1_brain_contrast_unknown"
    } else {
        "no_body_part"
    }
}

/// The sort's own "N not sorted" (2026-11-06): the stacks aliased `x` in
/// scope (`holds`, a condition on them) that no sort has judged, as a
/// condition, those outside the rules left out (a modality no sort has
/// judged a stack of, as a CT stack under the MRI pack:
/// [`crate::certainty::outside`]), since no sort will judge them. Its count
/// is the sorted step's `unsorted`, and a run of the sort over `without`
/// freezes these and no other.
pub(crate) fn unsorted_condition(store: &mut Store, holds: &str) -> Result<String, StoreError> {
    let (_, by) = crate::certainty::outside(store, holds)?;
    let mut condition = format!(
        "NOT EXISTS (SELECT 1 FROM {} cl WHERE cl.stack_id = x.id)",
        store.qualified("classification")
    );
    if !by.is_empty() {
        let modalities: Vec<String> = by.into_keys().collect();
        condition.push_str(&format!(
            " AND NOT EXISTS (SELECT 1 FROM {} f WHERE f.stack_id = x.id AND f.modality IN ({}))",
            store.qualified("stack_fingerprint"),
            quoted(&modalities)
        ));
    }
    Ok(condition)
}

/// What a run of the sort would go over (2026-11-06), as a model step's
/// `runs` says it: every scan it counts, or only the ones not sorted, those
/// outside the rules left out; one part each, since a sort is never split.
pub(crate) fn sort_runs(scans: i64, unsorted: i64) -> Value {
    json!({
        "all": {"scans": scans, "parts": parts(scans, None)},
        "without": {"scans": unsorted, "parts": parts(unsorted, None)},
    })
}

/// Ruling 35 (b) of 2026-10-16: body part and post-contrast are optional
/// models a person runs when the data need them, so each step says how many
/// of the scans in scope its model would add an answer to, and what that
/// is ([`without_condition`]). A count, never a value; read-only.
fn hint(store: &mut Store, step: &str, holds: &str) -> Result<Value, StoreError> {
    let scans = count(
        store,
        &format!(
            "SELECT COUNT(*) FROM {} x WHERE {holds} AND {}",
            store.qualified("stack"),
            without_condition(store, step)
        ),
    )?;
    Ok(json!({"kind": without_kind(step), "scans": scans}))
}

/// Which of the scans a step counts a run of it goes over (2026-11-05): every
/// one (`all`, as a run always went), or only those its hint counts, the
/// scans without its model's answer (`without`). Production's body part
/// said "135 with no body part" on a dataset of 456,132 scans, and its Run
/// went over all of them in 23 parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Over {
    All,
    Without,
}

impl Over {
    /// `all` or `without`, as a door's body and the command line say it.
    pub(crate) fn parse(words: &str) -> Result<Over, String> {
        match words.trim() {
            "" | "all" => Ok(Over::All),
            "without" => Ok(Over::Without),
            other => Err(format!(
                "scans is all (every scan the step counts) or without (only those without its answer), not {other}"
            )),
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Over::All => "all",
            Over::Without => "without",
        }
    }
}

/// Why a step's run is refused: the status a door answers it with, a short
/// reason a client tells refusals apart by, and the words a person reads.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Refused {
    pub(crate) status: u16,
    pub(crate) reason: &'static str,
    pub(crate) error: String,
}

impl Refused {
    fn new(status: u16, reason: &'static str, error: impl Into<String>) -> Refused {
        Refused {
            status,
            reason,
            error: error.into(),
        }
    }

    /// As a step of a summary carries it.
    fn doc(&self) -> Value {
        json!({"reason": self.reason, "error": self.error})
    }

    /// As the door answers it: the words, the reason and the step.
    pub(crate) fn reply(&self, step: &str) -> Reply {
        let mut r = Reply::error(self.status, self.error.clone());
        r.body["reason"] = json!(self.reason);
        r.body["step"] = json!(step);
        r
    }
}

/// What an operation's run is here: the pipeline by its label, and the
/// registered models it reads, by id, in the order its descriptor declares
/// its model inputs.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Plan {
    pub(crate) pipeline: String,
    pub(crate) models: Vec<i64>,
}

/// Each operation's run as it would be here, by its step's name.
pub(crate) type Plans = Vec<(&'static str, Result<Plan, Refused>)>;

/// The models served for a task, the one a run takes first: admitted or
/// promoted, the promoted ones first, then the newest; with `reads`, only
/// those that read that encoder.
fn served(store: &mut Store, task: &str, reads: Option<i64>) -> Result<Vec<Model>, StoreError> {
    let mut all: Vec<Model> = model::list(
        store,
        &model::Filter {
            task: Some(task),
            ..Default::default()
        },
    )?
    .into_iter()
    .rev()
    .filter(|m| m.answers())
    .filter(|m| reads.is_none_or(|e| m.encoder_model_ids.contains(&e)))
    .collect();
    // a stable sort keeps the newest first among each
    all.sort_by_key(|m| m.state != "promoted");
    Ok(all)
}

use nils_registry::model::is_cascade;

/// Body part's run: the newest active `bodypart-infer-fusion` and the
/// chosen model's parts (record 55 D4: the model the site chose for
/// `axis:body_part` in slot site, by default the one a recorded certificate
/// names, then the one promoted last, then the newest admitted single model),
/// in the order the descriptor declares them: the encoder the head names,
/// the head that answers `body_part`, and the coarse mode that answers
/// `body_region` from the same encoder (the one chosen for it where that
/// reads this encoder, its card naming the head where one does), which the
/// run goes without where none is served. Where the chosen model is a
/// cascade (record 55 E2), and it is one only once a person chose,
/// certified or promoted it (an admitted cascade waits), each input its
/// card names a part for gets that part, the input `cascade` gets the
/// cascade, and the coarse mode is the cascade's own part `coarse` and no
/// other, since the image refuses a coarse mode its cascade file does not
/// name; a cascade naming none is refused here. An optional input nothing
/// fills is left out.
fn body_part(store: &mut Store) -> Result<Result<Plan, Refused>, StoreError> {
    let refused =
        |reason: &'static str, error: String| -> Result<Result<Plan, Refused>, StoreError> {
            Ok(Err(Refused::new(409, reason, error)))
        };
    let Some(p) = nils_registry::pipeline::resolve(store, BODY_PART_PIPELINE)? else {
        return refused("no_pipeline", "no body-part pipeline is installed".into());
    };
    let d = match nils_pipeline::descriptor::from_value(p.descriptor.clone()) {
        Ok(d) => d,
        Err(e) => {
            return refused(
                "no_pipeline",
                format!(
                    "{}: the catalog's descriptor no longer checks: {e}",
                    p.label()
                ),
            );
        }
    };
    // record 55 D4: the chosen model answers; a cascade only once a person
    // chose, certified or promoted it, since choosing the body-part model
    // is a person's (the fast-path plan's 4.3)
    let Some(head) = model::chosen(store, "axis:body_part", "site")?.map(|c| c.model) else {
        let waiting = served(store, "axis:body_part", None)?
            .into_iter()
            .find(is_cascade)
            .map(|m| m.label());
        return refused(
            "no_model",
            match waiting {
                Some(c) => format!(
                    "no body-part model is served: the cascade {c} is admitted, and answers the step once it is chosen or promoted"
                ),
                None => "no body-part model is installed".into(),
            },
        );
    };
    // record 55 E2: a cascade names its parts, each by the input it fills,
    // and fills the input `cascade` itself; each part it names must be
    // registered, not retired, and admitted or promoted, as the model it
    // stands for would have to be
    let parts = model::parts(store, &head)?;
    let cascade = is_cascade(&head);
    if cascade {
        for (name, digest) in head.card["parts"].as_object().into_iter().flatten() {
            let why = match parts.iter().find(|(n, _)| n == name) {
                None => format!(
                    "{} names a part {name}, {}, that is not registered",
                    head.label(),
                    digest.as_str().unwrap_or("?")
                ),
                Some((_, m)) if m.state == "retired" => format!(
                    "{} reads its part {name}, {}, which is retired",
                    head.label(),
                    m.label()
                ),
                Some((_, m)) if !m.answers() => format!(
                    "{} reads its part {name}, {}, which is {} and neither admitted nor promoted",
                    head.label(),
                    m.label(),
                    m.state
                ),
                Some(_) => continue,
            };
            return refused("no_model", why);
        }
    }
    // a descriptor that takes no cascade (one older than it), or not each
    // of its parts, would run the plain path from the parts, or a cascade
    // without one: refused, so the run is never another model's
    if cascade {
        let takes: BTreeSet<&str> = d
            .inputs
            .iter()
            .filter(|t| t.ty == "model")
            .map(|t| t.id.as_str())
            .collect();
        let missing: Vec<&str> = std::iter::once("cascade")
            .chain(parts.iter().map(|(n, _)| n.as_str()))
            .filter(|n| !takes.contains(n))
            .collect();
        if !missing.is_empty() {
            return refused(
                "no_pipeline",
                format!(
                    "{} takes no model input {}, which the cascade {} is run with; add a descriptor version that takes it",
                    p.label(),
                    missing.join(", "),
                    head.label()
                ),
            );
        }
    }
    let part = |input: &str| {
        parts
            .iter()
            .find(|(name, _)| name == input)
            .map(|(_, m)| m.id)
    };
    let encoder = match head.encoder_model_id {
        Some(id) => model::get(store, id)?.filter(|m| m.state != "retired"),
        None => None,
    };
    let Some(encoder) = encoder else {
        return refused(
            "no_model",
            format!("the encoder {} reads is not registered", head.label()),
        );
    };
    // the coarse mode names the head it sums: one whose card names this
    // head first, else one of the same encoder (the image refuses a mode
    // file of another head). A cascade answers both modes with the coarse
    // part its file names, so it is given that part and no other: a mode
    // naming the cascade could never pass, since the cascade's file would
    // have to hold that mode's digest and the mode the cascade's
    let coarse = if !cascade {
        let modes = served(store, "axis:body_region", Some(encoder.id))?;
        // the chosen coarse mode where it names this head, since the image
        // takes only a mode of the head it sums; else as before
        let names_head = |m: &Model| m.card.to_string().contains(&head.digest);
        let chosen = model::chosen(store, "axis:body_region", "site")?
            .map(|c| c.model)
            .filter(|m| m.encoder_model_ids.contains(&encoder.id) && names_head(m));
        let named = modes.iter().find(|m| names_head(m)).map(|m| m.id);
        chosen
            .map(|m| m.id)
            .or(named)
            .or(modes.first().map(|m| m.id))
    } else {
        let Some(id) = part("coarse") else {
            return refused(
                "no_model",
                format!(
                    "{} names no coarse part: a cascade answers body_region with the coarse mode its file names",
                    head.label()
                ),
            );
        };
        Some(id)
    };
    let mut models = Vec::new();
    let mut skipped: Option<&str> = None;
    for t in d.inputs.iter().filter(|t| t.ty == "model") {
        // a cascade fills every input but its own from its parts: the
        // cascade is never its own head, nor its encoder the encoder
        if cascade && t.id != "cascade" && t.id != "coarse" && part(&t.id).is_none() {
            return refused(
                "no_model",
                format!(
                    "{} names no part {}, which {} reads",
                    head.label(),
                    t.id,
                    p.label()
                ),
            );
        }
        let given = match t.id.as_str() {
            "encoder" => part("encoder").or(Some(encoder.id)),
            "head" => part("head").or(Some(head.id)),
            "coarse" => coarse,
            "cascade" => cascade.then_some(head.id),
            other => match part(other) {
                Some(id) => Some(id),
                None if t.optional => None,
                None => {
                    return refused(
                        "no_pipeline",
                        format!(
                            "{} reads a model input {other} the body-part step does not fill",
                            p.label()
                        ),
                    );
                }
            },
        };
        // models are given in the order of the inputs, so one left out
        // can only be the last
        match (given, skipped) {
            (Some(_), Some(gone)) => {
                return refused(
                    "no_model",
                    format!(
                        "{} reads {} after {gone}, for which no model is served",
                        p.label(),
                        t.id
                    ),
                );
            }
            (Some(id), None) => models.push(id),
            (None, _) if t.optional => skipped = Some(t.id.as_str()),
            (None, _) => {
                return refused(
                    "no_model",
                    format!("no model is served for {}'s input {}", p.label(), t.id),
                );
            }
        }
    }
    Ok(Ok(Plan {
        pipeline: p.label(),
        models,
    }))
}

/// Post-contrast's run (record 55 E3): `pipeline` (the cascade's, or the
/// full pipeline's) given the chosen post-contrast cascade and its parts
/// (record 55 D4). The cascade answers the step only once a person chose,
/// certified or promoted it
/// (the speed plan's choice, as body part's); each part its card names
/// must be registered, not retired, and admitted or promoted; each model
/// input of the descriptor is filled by the part of its name, and the input
/// `cascade` by the cascade itself; an optional input nothing fills is left
/// out, and a required one refuses the run.
fn post_contrast(store: &mut Store, pipeline: &str) -> Result<Result<Plan, Refused>, StoreError> {
    let refused =
        |reason: &'static str, error: String| -> Result<Result<Plan, Refused>, StoreError> {
            Ok(Err(Refused::new(409, reason, error)))
        };
    let Some(p) = nils_registry::pipeline::resolve(store, pipeline)? else {
        return refused(
            "no_pipeline",
            format!("no post-contrast pipeline {pipeline} is installed"),
        );
    };
    let d = match nils_pipeline::descriptor::from_value(p.descriptor.clone()) {
        Ok(d) => d,
        Err(e) => {
            return refused(
                "no_pipeline",
                format!(
                    "{}: the catalog's descriptor no longer checks: {e}",
                    p.label()
                ),
            );
        }
    };
    // record 55 D4: the chosen model, a cascade, answers; an admitted
    // cascade only once a person chose, certified or promoted it
    let cascade = model::chosen(store, "axis:post_contrast", "site")?
        .map(|c| c.model)
        .filter(is_cascade);
    let Some(head) = cascade else {
        let candidates = served(store, "axis:post_contrast", None)?;
        let waiting = candidates.iter().find(|m| is_cascade(m)).map(Model::label);
        return refused(
            "no_model",
            match waiting {
                Some(c) => format!(
                    "no post-contrast model is served: the cascade {c} is admitted, and answers the step once it is chosen or promoted"
                ),
                None => "no post-contrast model is installed".into(),
            },
        );
    };
    let parts = model::parts(store, &head)?;
    for (name, digest) in head.card["parts"].as_object().into_iter().flatten() {
        let why = match parts.iter().find(|(n, _)| n == name) {
            None => format!(
                "{} names a part {name}, {}, that is not registered",
                head.label(),
                digest.as_str().unwrap_or("?")
            ),
            Some((_, m)) if m.state == "retired" => format!(
                "{} reads its part {name}, {}, which is retired",
                head.label(),
                m.label()
            ),
            Some((_, m)) if !m.answers() => format!(
                "{} reads its part {name}, {}, which is {} and neither admitted nor promoted",
                head.label(),
                m.label(),
                m.state
            ),
            Some(_) => continue,
        };
        return refused("no_model", why);
    }
    let mut models = Vec::new();
    let mut skipped: Option<&str> = None;
    for t in d.inputs.iter().filter(|t| t.ty == "model") {
        let given = match t.id.as_str() {
            "cascade" => Some(head.id),
            other => parts.iter().find(|(n, _)| n == other).map(|(_, m)| m.id),
        };
        match (given, skipped) {
            (Some(_), Some(gone)) => {
                return refused(
                    "no_model",
                    format!(
                        "{} reads {} after {gone}, which {} names no part for",
                        p.label(),
                        t.id,
                        head.label()
                    ),
                );
            }
            (Some(id), None) => models.push(id),
            (None, _) if t.optional => skipped = Some(t.id.as_str()),
            (None, _) => {
                return refused(
                    "no_model",
                    format!(
                        "{} names no part {}, which {} reads",
                        head.label(),
                        t.id,
                        p.label()
                    ),
                );
            }
        }
    }
    Ok(Ok(Plan {
        pipeline: p.label(),
        models,
    }))
}

/// System 1's run (record 55 D3, spec §11.1): the newest active
/// `system1-score` given the model chosen for the task `system1` (record
/// 55 D4), a site model; refused where none is registered, admitted or
/// chosen, and where the pipeline is not in the catalog.
fn system1(store: &mut Store) -> Result<Result<Plan, Refused>, StoreError> {
    let refused =
        |reason: &'static str, error: String| -> Result<Result<Plan, Refused>, StoreError> {
            Ok(Err(Refused::new(409, reason, error)))
        };
    let Some(p) = nils_registry::pipeline::resolve(store, crate::system1::PIPELINE)? else {
        return refused(
            "no_pipeline",
            format!(
                "no System 1 pipeline is installed: {} is added from the engine's pipelines folder",
                crate::system1::PIPELINE
            ),
        );
    };
    let d = match nils_pipeline::descriptor::from_value(p.descriptor.clone()) {
        Ok(d) => d,
        Err(e) => {
            return refused(
                "no_pipeline",
                format!(
                    "{}: the catalog's descriptor no longer checks: {e}",
                    p.label()
                ),
            );
        }
    };
    let Some(chosen) = nils_registry::opinion::chosen(store)? else {
        let any = model::list(
            store,
            &model::Filter {
                task: Some(nils_registry::opinion::TASK),
                ..Default::default()
            },
        )?;
        return refused(
            "no_model",
            if any.is_empty() {
                "no System 1 model is installed: it is a site model, registered for the task system1 and admitted before it runs".to_string()
            } else {
                "no System 1 model is admitted, promoted or chosen: a registered one answers once it is admitted".to_string()
            },
        );
    };
    if d.inputs.iter().filter(|t| t.ty == "model").count() != 1 {
        return refused(
            "no_pipeline",
            format!(
                "{} reads {} models; System 1's pipeline reads one, the chosen model",
                p.label(),
                d.inputs.iter().filter(|t| t.ty == "model").count()
            ),
        );
    }
    Ok(Ok(Plan {
        pipeline: p.label(),
        models: vec![chosen.model.id],
    }))
}

/// After a run of the post-contrast cascade (record 55 E3): the stacks
/// whose table says `deferred` go to the full pipeline, frozen into a
/// handle and queued as a job of the same step and for the same scope as
/// the cascade's own job, where the full pipeline is installed and its
/// plan holds; otherwise they stay "not sure" and the summary says why.
pub(crate) fn fallback(
    registry: &mut Registry,
    run_id: i64,
    job_id: i64,
    principal: &str,
) -> Value {
    let result = (|| -> Result<Value, String> {
        let epoch = registry.meta().epoch;
        let store = registry.store();
        let d = store.dialect();
        let sql = format!(
            "SELECT DISTINCT m.stack_id, se.subject_id FROM {} m JOIN {} x ON x.id = m.stack_id \
             JOIN {} se ON se.id = x.series_id WHERE m.run_id = {} AND m.name = 'reason' \
             AND m.text = 'deferred' ORDER BY m.stack_id",
            store.qualified("measure"),
            store.qualified("stack"),
            store.qualified("series"),
            d.param(1, nils_registry::schema::Type::Int)
        );
        let rows: Vec<Row> = store
            .query(&sql, &[nils_registry::store::Param::Int(run_id)])
            .map_err(|e| e.to_string())?
            .iter()
            .map(|r| {
                Ok(Row(vec![
                    Cell::Int(r.int(0)?),
                    r.opt_int(1)?.map_or(Cell::Null, Cell::Int),
                ]))
            })
            .collect::<Result<_, StoreError>>()
            .map_err(|e| e.to_string())?;
        if rows.is_empty() {
            return Ok(json!({"deferred": 0}));
        }
        let deferred = rows.len();
        let plan = match post_contrast(store, POST_CONTRAST_FULL).map_err(|e| e.to_string())? {
            Ok(plan) => plan,
            Err(refused) => {
                return Ok(json!({
                    "deferred": deferred, "queued": false, "why": refused.error,
                }));
            }
        };
        let (handle, frozen) = save_stacks(store, rows, principal, epoch, &Pinned::default())?;
        let mut command = vec![
            "run".to_string(),
            plan.pipeline,
            "--handle".to_string(),
            handle.to_string(),
        ];
        for m in &plan.models {
            command.extend(["--model".to_string(), m.to_string()]);
        }
        // the cascade's job says which step and scope it ran for
        let sql = format!(
            "SELECT {}, name FROM {} WHERE id = {}",
            crate::text_of(store, "job", "args"),
            store.qualified("job"),
            d.param(1, nils_registry::schema::Type::Int)
        );
        let (args, target) = match store
            .query_opt(&sql, &[nils_registry::store::Param::Int(job_id)])
            .map_err(|e| e.to_string())?
        {
            Some(r) => (
                r.opt_text(0)
                    .ok()
                    .flatten()
                    .and_then(|t| serde_json::from_str::<Value>(t).ok())
                    .unwrap_or(Value::Null),
                r.opt_text(1).ok().flatten().map(str::to_string),
            ),
            None => (Value::Null, None),
        };
        // under the grants, detail and principal the cascade's job ran with
        let mut extra = match args {
            Value::Object(mut m) => {
                m.remove("argv");
                m.remove("queued");
                Value::Object(m)
            }
            _ => json!({}),
        };
        extra["step"] = json!("post_contrast");
        extra["fallback_of"] = json!(run_id);
        let id = nils_registry::job::enqueue_with(
            store,
            &command,
            target.as_deref(),
            Some(principal),
            extra,
        )
        .map_err(|e| e.to_string())?;
        Ok(json!({
            "deferred": deferred, "queued": true, "job": id, "handle": handle, "stacks": frozen,
        }))
    })();
    result.unwrap_or_else(|e| json!({"error": e}))
}

/// What an operation's run would be here, or why it is refused: a pipeline
/// in the catalog runs it, the models it reads are served, and pipelines
/// run here, with a container runtime and a working place.
pub(crate) fn plan(
    registry: &mut Registry,
    step: &str,
) -> Result<Result<Plan, Refused>, StoreError> {
    let planned = match step {
        "body_part" => body_part(registry.store())?,
        // record 55 E3: model r1 through its cascade
        "post_contrast" => post_contrast(registry.store(), POST_CONTRAST_PIPELINE)?,
        // record 55 D3: System 1, the chosen site model of its task
        crate::system1::STEP => system1(registry.store())?,
        other => Err(Refused::new(
            404,
            "no_step",
            format!("{other} is not a step a door runs; body_part, post_contrast and system1 are"),
        )),
    };
    let plan = match planned {
        Ok(plan) => plan,
        Err(refused) => return Ok(Err(refused)),
    };
    let found = crate::pipelines::detect_cached(registry);
    if found.runtime.is_none() {
        let why = found
            .reason
            .unwrap_or_else(|| "pipelines are off: no container runtime here".to_string());
        return Ok(Err(Refused::new(409, "no_runtime", why)));
    }
    if let Err(why) = crate::pipelines::run_places(registry.store()) {
        return Ok(Err(Refused::new(409, "no_place", why)));
    }
    Ok(Ok(plan))
}

/// Every operation's run as it would be here, by its step's name, for a
/// summary that says of each step why a run of it would be refused.
pub(crate) fn plans(registry: &mut Registry) -> Result<Plans, StoreError> {
    OPERATIONS
        .iter()
        .map(|(name, _)| Ok((*name, plan(registry, name)?)))
        .collect()
}

/// One run of an operation over the scans, as its step reads it.
struct Ran {
    id: i64,
    job: Option<i64>,
    status: String,
    started_at: Option<String>,
    finished_at: Option<String>,
}

/// One job a door queued for an operation's run over a dataset or a cohort.
struct Asked {
    id: i64,
    state: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    progress: Option<Value>,
}

impl Asked {
    fn open(&self) -> bool {
        matches!(self.state.as_str(), "queued" | "running" | "cancelling")
    }
}

/// The pipelines in the catalog whose proposals answer one of the axes, of
/// any state, and whether one of them is active.
fn proposing(store: &mut Store, axes: &[&str]) -> Result<(Vec<i64>, bool), StoreError> {
    let all =
        nils_registry::pipeline::list(store).map_err(|e| StoreError::Message(e.to_string()))?;
    let mut ids = Vec::new();
    let mut active = false;
    for p in all {
        let proposes = p.descriptor["x-nils"]["proposals"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|x| x["axis"].as_str().is_some_and(|a| axes.contains(&a)));
        if proposes {
            active |= p.state == "active";
            ids.push(p.id);
        }
    }
    Ok((ids, active))
}

/// The newest runs of the pipelines over a stack in scope, newest first.
fn runs_over(
    store: &mut Store,
    pipelines: &[i64],
    scope: Scope<'_>,
) -> Result<Vec<Ran>, StoreError> {
    if pipelines.is_empty() {
        return Ok(Vec::new());
    }
    let holds = scope.holds(store);
    let sql = format!(
        "SELECT r.id, r.job_id, r.status, {}, {} FROM {} r \
         WHERE r.pipeline_id IN ({}) AND r.handle_id IS NOT NULL \
         AND EXISTS (SELECT 1 FROM {} hm JOIN {} x ON x.id = hm.key \
         WHERE hm.handle_id = r.handle_id AND {holds}) ORDER BY r.id DESC LIMIT {RUNS}",
        crate::text_of(store, "pipeline_run", "started_at"),
        crate::text_of(store, "pipeline_run", "finished_at"),
        store.qualified("pipeline_run"),
        list(pipelines),
        store.qualified("handle_member"),
        store.qualified("stack"),
    );
    store
        .query(&sql, &[])?
        .iter()
        .map(|r| {
            Ok(Ran {
                id: r.int(0)?,
                job: r.opt_int(1)?,
                status: r.text(2)?.to_string(),
                started_at: r.opt_text(3)?.map(str::to_string),
                finished_at: r.opt_text(4)?.map(str::to_string),
            })
        })
        .collect()
}

/// The jobs a door queued for a step's run over a dataset or a cohort
/// (`for`, `dataset:<name>` or `cohort:<name>`), newest first, among the
/// newest pipeline jobs. The args are read here rather than matched in SQL,
/// since the two backends render JSON differently.
fn asked(store: &mut Store, step: &str, target: &str) -> Result<Vec<Asked>, StoreError> {
    let sql = format!(
        "SELECT id, state, {}, {}, {}, {} FROM {} WHERE kind = 'pipeline' \
         ORDER BY id DESC LIMIT {ASKED}",
        crate::text_of(store, "job", "started_at"),
        crate::text_of(store, "job", "finished_at"),
        crate::text_of(store, "job", "progress"),
        crate::text_of(store, "job", "args"),
        store.qualified("job"),
    );
    let json = |t: Option<&str>| t.and_then(|t| serde_json::from_str::<Value>(t).ok());
    let mut out = Vec::new();
    for r in store.query(&sql, &[])? {
        let args = json(r.opt_text(5)?).unwrap_or(Value::Null);
        if args["step"].as_str() != Some(step) || args["for"].as_str() != Some(target) {
            continue;
        }
        out.push(Asked {
            id: r.int(0)?,
            state: r.text(1)?.to_string(),
            started_at: r.opt_text(2)?.map(str::to_string),
            finished_at: r.opt_text(3)?.map(str::to_string),
            progress: json(r.opt_text(4)?),
        });
    }
    Ok(out)
}

/// The jobs of the operations' runs over a dataset's scans, for its log.
pub(crate) fn run_jobs(store: &mut Store, scope: Scope<'_>) -> Result<BTreeSet<i64>, StoreError> {
    let mut out = BTreeSet::new();
    for (_, axes) in OPERATIONS {
        let (pipelines, _) = proposing(store, axes)?;
        for r in runs_over(store, &pipelines, scope)? {
            out.extend(r.job);
        }
    }
    Ok(out)
}

/// Where an operation's run over a scope's scans is: its runs over them
/// and the jobs a door queued for it, each newest first.
struct Attempts {
    runs: Vec<Ran>,
    asked: Vec<Asked>,
}

impl Attempts {
    /// A step's attempts over a scope's scans, by the pipelines that
    /// propose its axes.
    fn of(
        store: &mut Store,
        step: &str,
        pipelines: &[i64],
        scope: Scope<'_>,
        target: &str,
    ) -> Result<Attempts, StoreError> {
        Ok(Attempts {
            runs: runs_over(store, pipelines, scope)?,
            asked: asked(store, step, target)?,
        })
    }

    /// The run over the scans that runs now, or whose engine went away and
    /// which the lane takes up.
    fn going(&self) -> Option<&Ran> {
        self.runs
            .iter()
            .find(|r| matches!(r.status.as_str(), "running" | "interrupted"))
    }

    /// A job a door queued for the step that is not over.
    fn open(&self) -> Option<&Asked> {
        self.asked.iter().find(|j| j.open())
    }

    /// The job of a run queued or running now, none where nothing is.
    fn busy(&self) -> Option<Option<i64>> {
        self.going()
            .map(|r| r.job)
            .or_else(|| self.open().map(|j| Some(j.id)))
    }

    /// The newest job a door queued for the step that made no run, where
    /// it is newer than every run over the scans: one still waiting, or one
    /// that ended before its run began.
    fn unmade(&self) -> Option<&Asked> {
        let newest = self.runs.first().and_then(|r| r.job);
        self.asked
            .first()
            .filter(|j| newest.is_none_or(|run| j.id > run))
    }

    /// Whether the newest attempt failed: the door's job that made no run,
    /// or else the newest run. A cancelled one is no failure.
    fn failed(&self) -> bool {
        match self.unmade() {
            Some(j) => j.state == "failed",
            None => self.runs.first().is_some_and(|r| r.status == "failed"),
        }
    }

    /// Every job of the step over the scans, newest first.
    fn jobs(&self) -> Vec<i64> {
        let mut all: Vec<i64> = self
            .runs
            .iter()
            .filter_map(|r| r.job)
            .chain(self.asked.iter().map(|j| j.id))
            .collect();
        all.sort_unstable_by(|a, b| b.cmp(a));
        all.dedup();
        all
    }
}

/// A going run's progress on its step (2026-11-05): the units over of the
/// units, as the unit rows count them, with the run's own last beat laid
/// over them (`crate::pipelines::live_progress`: the count a person reads
/// first, a container's own where it says, the part, since when). A beat
/// whose count is not whole leaves the rows' count. None while the run has
/// no units yet and no beat says anything.
fn with_live(mut doc: Value, live: Value, has_units: bool) -> Option<Value> {
    let Value::Object(live) = live else {
        return has_units.then_some(doc);
    };
    for (k, v) in live {
        // the lane's figures are the run page's, not a step's, and so are
        // each part's own beat and run where parts run at once (2026-11-06)
        if matches!(k.as_str(), "lane" | "each" | "runs")
            || (matches!(k.as_str(), "done" | "total") && !v.is_u64())
        {
            continue;
        }
        doc[k] = v;
    }
    Some(doc)
}

/// Why a failed step failed: the failure of the door's job that made no
/// run, where the newest attempt was one, else the newest run's, as the
/// jobs door and the run door say them (`{kind, cause, advice, fix}`, with
/// a memory cap's `said` and `cap_gb`); the engine's own words, never a
/// tool's, so they are shown at any detail. Null where neither is read.
fn failure_of(store: &mut Store, at: &Attempts) -> Value {
    if let Some(j) = at.unmade().filter(|j| j.state == "failed") {
        return nils_registry::job::show(store, j.id)
            .ok()
            .flatten()
            .map_or(Value::Null, |j| {
                crate::failures::job_doc(&j)["failure"].clone()
            });
    }
    at.runs
        .first()
        .filter(|r| r.status == "failed")
        .and_then(|r| nils_registry::pipeline::run(store, r.id).ok().flatten())
        .and_then(|r| serde_json::to_value(&r).ok())
        .map_or(Value::Null, |doc| crate::failures::of_run_doc(&doc))
}

/// Why a step's run over a scope's scans is refused now, or none: a run
/// of it over them is queued or runs, nothing here runs it, or there are
/// no scans to run it over.
fn refusal(
    step: &str,
    busy: Option<Option<i64>>,
    plan: &Result<Plan, Refused>,
    scans: i64,
) -> Option<Refused> {
    if let Some(job) = busy {
        let words = match job {
            Some(job) => format!(
                "{} runs already over these scans, as job {job}",
                title(step)
            ),
            None => format!("{} runs already over these scans", title(step)),
        };
        return Some(Refused::new(409, "running", words));
    }
    match plan {
        Err(refused) => Some(refused.clone()),
        Ok(_) if scans <= 0 => Some(Refused::new(
            409,
            "no_scans",
            "there are no scans to run it over yet",
        )),
        Ok(_) => None,
    }
}

/// Each operation's step over the scans in scope, `scans` of them, for a
/// dataset or a cohort (`target`, as a door's job names what it is for):
/// its state (queued while a door's job for it waits, or its run's engine
/// went away and the lane takes it up; running while that job or a run over
/// the scans goes on; failed when the newest of those failed; done once one
/// finished or its model answered one of them; waiting while it is served
/// and has not run; off where nothing serves it), the run or the job with
/// its times, the units over while it runs, how many scans its model
/// answered at or above its threshold, how many wait on a person, the jobs
/// of its runs and of the door over them, newest first, and why a run of
/// it would be refused now (`refusal`, none where the door would queue it).
pub(crate) fn steps(
    store: &mut Store,
    scope: Scope<'_>,
    target: &str,
    scans: i64,
    plans: &Plans,
    hide_sealed: bool,
) -> Result<Vec<Value>, StoreError> {
    let names: Vec<&str> = plans.iter().map(|(n, _)| *n).collect();
    let counted = counted(store, scope, hide_sealed, &names)?;
    steps_counted(store, scope, target, scans, plans, &counted)
}

/// What an operation's step counts over the scans in scope, which a run, a
/// sort or a person's act changes and the run's own progress never does:
/// the scans its model answered, the ones a question about its axes waits
/// on a person for, the ones its model would add an answer to (`hint`),
/// and post-contrast's own label. Read apart from where its run is, so that
/// a page asked again and again may keep them while the run's state and
/// progress are read every time (the dataset's summary, 2026-10-17).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Counted {
    name: &'static str,
    answered: i64,
    look: i64,
    hint: Value,
    label: Option<Value>,
}

/// The counts of the operations named, over the scans in scope.
impl Counted {
    /// The count as the registry keeps a dataset's counts between starts of
    /// the engine (`dataset_summary`): a document of its fields.
    pub(crate) fn stored(&self) -> Value {
        json!({
            "name": self.name,
            "answered": self.answered,
            "look": self.look,
            "hint": self.hint,
            "label": self.label,
        })
    }

    /// A count kept so, read back; none for an operation this engine has
    /// not, or a document that is no count.
    pub(crate) fn from_stored(stored: &Value) -> Option<Counted> {
        let name = OPERATIONS
            .iter()
            .map(|(name, _)| *name)
            .find(|name| Some(*name) == stored["name"].as_str())?;
        Some(Counted {
            name,
            answered: stored["answered"].as_i64()?,
            look: stored["look"].as_i64()?,
            hint: stored["hint"].clone(),
            label: match &stored["label"] {
                Value::Null => None,
                label => Some(label.clone()),
            },
        })
    }
}

pub(crate) fn counted(
    store: &mut Store,
    scope: Scope<'_>,
    hide_sealed: bool,
    names: &[&str],
) -> Result<Vec<Counted>, StoreError> {
    let holds = shown(store, scope.holds(store), hide_sealed);
    let (member, item, stack) = (
        store.qualified("review_member"),
        store.qualified("review_item"),
        store.qualified("stack"),
    );
    let mut out = Vec::new();
    for (name, axes) in OPERATIONS {
        if !names.contains(&name) {
            continue;
        }
        let groups: Vec<String> = axes.iter().map(|a| format!("{a}:model")).collect();
        // the scans its model answered at or above its threshold: staged,
        // or put in force since
        let answered = count(
            store,
            &format!(
                "SELECT COUNT(DISTINCT rm.stack_id) FROM {member} rm JOIN {item} ri ON ri.id = rm.item_id \
                 JOIN {stack} x ON x.id = rm.stack_id WHERE ri.kind IN ({}) \
                 AND ri.status IN ('staged', 'accepted') AND {holds}",
                quoted(&groups)
            ),
        )?;
        // the scans a question about its axes waits on a person for: what
        // its model was unsure of, a pass's about them, and what a sort
        // asked before record 56, by who asks it (`certainty`)
        let look = crate::certainty::Asks::of(store, "'open'", &|a| {
            a == crate::certainty::Asker::Operation(name)
        })?
        .count(store, &holds)?
        .scans;
        // ruling 35 (b): how many scans its model would add an answer to
        let hint = hint(store, name, &holds)?;
        // record 55 F5: post-contrast's own label, the time rule's, on the
        // scans in scope
        let label = if name == "post_contrast" {
            Some(crate::contrast::counts(store, &holds)?)
        } else {
            None
        };
        out.push(Counted {
            name,
            answered,
            look,
            hint,
            label,
        });
    }
    Ok(out)
}

/// [`steps`] with each operation's counts read already ([`counted`]): where
/// its run is, its job, its progress and why a run would be refused, read
/// now.
pub(crate) fn steps_counted(
    store: &mut Store,
    scope: Scope<'_>,
    target: &str,
    scans: i64,
    plans: &Plans,
    counted: &[Counted],
) -> Result<Vec<Value>, StoreError> {
    let (model, unit) = (store.qualified("model"), store.qualified("pipeline_unit"));
    let mut out = Vec::new();
    for (name, axes) in OPERATIONS {
        // a step no plan is given for is not asked about: `step_of` asks
        // about one
        let Some((_, planned)) = plans.iter().find(|(n, _)| *n == name) else {
            continue;
        };
        let Some(c) = counted.iter().find(|c| c.name == name) else {
            continue;
        };
        let (answered, look) = (c.answered, c.look);
        let (pipelines, active) = proposing(store, axes)?;
        let tasks: Vec<String> = axes.iter().map(|a| format!("axis:{a}")).collect();
        let models = count(
            store,
            &format!(
                "SELECT COUNT(*) FROM {model} WHERE task IN ({}) AND state IN ('admitted', 'promoted')",
                quoted(&tasks)
            ),
        )?;
        let served = active && models > 0;
        let at = Attempts::of(store, name, &pipelines, scope, target)?;
        let going = at.going();
        let open = at.open();
        let finished = at
            .runs
            .iter()
            .any(|r| matches!(r.status.as_str(), "done" | "partial"));
        let state = match (going.map(|r| r.status.as_str()), open) {
            (Some("running"), _) => "running",
            // the engine that ran it went away, and the lane takes it up
            (Some(_), _) => "queued",
            (None, Some(j)) if j.state == "queued" => "queued",
            // its job has started, and its run's row comes next
            (None, Some(_)) => "running",
            (None, None) if at.failed() => "failed",
            (None, None) if finished || answered > 0 => "done",
            (None, None) if served => "waiting",
            (None, None) => "off",
        };
        // the attempt the step's times are of: the run going on, the door's
        // job not over, a door's job that failed before its run began, or
        // the newest run
        let job_only = open.or_else(|| at.unmade().filter(|j| j.state == "failed"));
        let (job, run, started_at, finished_at) = match (going, job_only) {
            (Some(r), _) => (
                r.job,
                Some(r.id),
                r.started_at.clone(),
                r.finished_at.clone(),
            ),
            (None, Some(j)) => (
                Some(j.id),
                None,
                j.started_at.clone(),
                j.finished_at.clone(),
            ),
            (None, None) => match at.runs.first() {
                Some(r) => (
                    r.job,
                    Some(r.id),
                    r.started_at.clone(),
                    r.finished_at.clone(),
                ),
                None => (None, None, None, None),
            },
        };
        let progress = match (going, open) {
            (Some(r), _) => {
                let rows = store.query(
                    &format!(
                        "SELECT COUNT(*), COALESCE(SUM(CASE WHEN state = 'over' THEN 1 ELSE 0 END), 0) \
                         FROM {unit} WHERE run_id = {}",
                        r.id
                    ),
                    &[],
                )?;
                let (total, over) = (rows[0].int(0)?, rows[0].int(1)?);
                // 2026-11-05: the run's own beat over the unit rows' count,
                // so a unit that holds many stacks moves while it works
                let live = crate::pipelines::live_progress(store, r.id, r.job, &r.status);
                with_live(json!({"done": over, "total": total}), live, total > 0)
            }
            (None, Some(j)) => j.progress.clone(),
            (None, None) => None,
        };
        // 2026-11-05: a failed step says why, in the engine's words for its
        // failure, beside the run (or the job, where none was made) it was
        let failure = if state == "failed" {
            failure_of(store, &at)
        } else {
            Value::Null
        };
        let refused = refusal(name, at.busy(), planned, scans);
        // what a run of it would go over, before it starts (2026-11-05): every
        // scan it counts, or the ones without its answer that its hint
        // counts, each with the parts it would run in
        let runs = match planned {
            Ok(p) => {
                let size = part_size(store, &p.pipeline);
                let mut runs = json!({"all": {"scans": scans, "parts": parts(scans, size)}});
                if let Some(n) = c.hint["scans"].as_i64() {
                    runs["without"] = json!({"scans": n, "parts": parts(n, size)});
                }
                runs
            }
            Err(_) => Value::Null,
        };
        out.push(json!({
            "step": name,
            "state": state,
            "job": job,
            "run": run,
            "started_at": started_at,
            "finished_at": finished_at,
            "progress": progress,
            "failure": failure,
            "served": served,
            "answered": answered,
            "look": look,
            "of": scans,
            "jobs": at.jobs(),
            "refusal": refused.map(|r| r.doc()),
            "label": c.label,
            "hint": c.hint,
            "runs": runs,
        }));
    }
    Ok(out)
}

/// One operation's step over a dataset's or a cohort's scans, as [`steps`]
/// gives it on their pages (the dataset's summary, the cohort's document),
/// for a page that shows the operation's answers: the Main scans map's
/// layers (record 55, decision 6). Its state, its run and job, what its
/// model answered, how many of those answers are staged and not yet put in
/// force (`staged`, which the layer does not show yet), and why a run of
/// it would be refused now; none for a step that is no operation of its
/// own.
pub(crate) fn step_of(
    registry: &mut Registry,
    scope: Scope<'_>,
    target: &str,
    step: &str,
    hide_sealed: bool,
) -> Result<Option<Value>, StoreError> {
    let Some((name, axes)) = OPERATIONS.iter().find(|(n, _)| *n == step) else {
        return Ok(None);
    };
    let plans: Plans = vec![(*name, plan(registry, name)?)];
    let store = registry.store();
    let holds = shown(store, scope.holds(store), hide_sealed);
    let scans = count(
        store,
        &format!(
            "SELECT COUNT(*) FROM {} x WHERE {holds}",
            store.qualified("stack")
        ),
    )?;
    // the scans its model answered whose answers are staged and not yet put
    // in force: counted in `answered`, but no part of the stored axes a
    // layer reads, so a page that shows the layer says how many wait
    let groups: Vec<String> = axes.iter().map(|a| format!("{a}:model")).collect();
    let staged = count(
        store,
        &format!(
            "SELECT COUNT(DISTINCT rm.stack_id) FROM {} rm JOIN {} ri ON ri.id = rm.item_id \
             JOIN {} x ON x.id = rm.stack_id WHERE ri.kind IN ({}) AND ri.status = 'staged' \
             AND {holds}",
            store.qualified("review_member"),
            store.qualified("review_item"),
            store.qualified("stack"),
            quoted(&groups)
        ),
    )?;
    Ok(steps(store, scope, target, scans, &plans, hide_sealed)?
        .into_iter()
        .next()
        .map(|mut doc| {
            doc["staged"] = json!(staged);
            doc
        }))
}

/// A cohort's steps, as its document carries them: its members' scans
/// sorted, with what the sort asks of them, then each operation's step
/// over them. The main scans of its members are the picks summary's.
pub(crate) fn of_cohort(
    registry: &mut Registry,
    cohort: i64,
    name: &str,
    hide_sealed: bool,
) -> Result<Vec<Value>, StoreError> {
    let plans = plans(registry)?;
    let store = registry.store();
    let scope = Scope::Cohort(cohort);
    let holds = shown(store, scope.holds(store), hide_sealed);
    let (stack, class) = (store.qualified("stack"), store.qualified("classification"));
    let scans = count(
        store,
        &format!("SELECT COUNT(*) FROM {stack} x WHERE {holds}"),
    )?;
    let sorted = count(
        store,
        &format!(
            "SELECT COUNT(*) FROM {stack} x WHERE {holds} \
             AND EXISTS (SELECT 1 FROM {class} cl WHERE cl.stack_id = x.id)"
        ),
    )?;
    // a look as the card counts one, and a pass's questions beside it
    let look = crate::certainty::Asks::sort(store)?
        .count(store, &holds)?
        .scans;
    let passes = crate::certainty::passes(store, &holds)?;
    // a stack outside the rules (a CT stack under the MRI pack) is never
    // sorted and never waits for it (2026-11-05)
    let (outside, outside_by) = crate::certainty::outside(store, &holds)?;
    let unsorted = (scans - sorted - outside).max(0);
    let mut out = vec![json!({
        "step": "sorted",
        "state": if scans > 0 && sorted + outside >= scans { "done" } else { "waiting" },
        "job": null,
        "started_at": null,
        "finished_at": null,
        "progress": null,
        "scans": sorted,
        "of": scans,
        "look": look,
        "passes": passes,
        "unsorted": unsorted,
        "outside": outside,
        "outside_by": outside_by,
        // 2026-11-06: what a sort would go over, every scan or the ones
        // not sorted
        "runs": sort_runs(scans, unsorted),
    })];
    out.extend(steps(
        store,
        scope,
        &format!("cohort:{name}"),
        scans,
        &plans,
        hide_sealed,
    )?);
    Ok(out)
}

/// Freeze the scans in scope into a handle a run pins, each with its
/// subject, in stack order: every stack the step counts, a sample sealed
/// now among them, as a selection frozen for a run keeps them (record 48,
/// D1 of the move), so the handle opens only to a scope that reads sealed
/// stacks; with `over`, only the stacks that meet it as well (a step's
/// scans without its answer, 2026-11-05). A selection's handle names the
/// selection's version it was frozen from (`pinned`, 2026-11-06). Answers
/// the handle and how many stacks it holds.
fn freeze(
    store: &mut Store,
    scope: Scope<'_>,
    over: Option<&str>,
    principal: &str,
    epoch: i64,
    pinned: &Pinned<'_>,
) -> Result<(i64, usize), String> {
    let holds = scope.holds(store);
    // 2026-11-05: or only those a step counts without its answer
    let only = over.map(|c| format!(" AND {c}")).unwrap_or_default();
    let sql = format!(
        "SELECT x.id, se.subject_id FROM {} x JOIN {} se ON se.id = x.series_id \
         WHERE {holds}{only} ORDER BY x.id",
        store.qualified("stack"),
        store.qualified("series"),
    );
    let rows: Vec<Row> = store
        .query(&sql, &[])
        .map_err(|e| e.to_string())?
        .iter()
        .map(|r| {
            Ok(Row(vec![
                Cell::Int(r.int(0)?),
                r.opt_int(1)?.map_or(Cell::Null, Cell::Int),
            ]))
        })
        .collect::<Result<_, StoreError>>()
        .map_err(|e| e.to_string())?;
    save_stacks(store, rows, principal, epoch, pinned)
}

/// What a handle of stacks frozen by the engine records of where they came
/// from: its pins (a scope's main scans name the version of the rules they
/// were read under, a selection's scans the selection's version), the pack
/// and the scheme. None of these for a dataset's or a cohort's scans, which
/// are every scan in scope.
#[derive(Debug, Default)]
pub(crate) struct Pinned<'a> {
    pub(crate) versions: Value,
    pub(crate) pack_version: Option<&'a str>,
    pub(crate) scheme_digest: Option<&'a str>,
}

/// Save rows of a stack and its subject, in the order given, as a handle a
/// run pins, under the hash the ask gives an answer. Answers the handle and
/// how many stacks it holds.
pub(crate) fn save_stacks(
    store: &mut Store,
    rows: Vec<Row>,
    principal: &str,
    epoch: i64,
    pinned: &Pinned<'_>,
) -> Result<(i64, usize), String> {
    // the hash the ask gives an answer: each row's cells as text
    let hash = {
        use blake2::Digest as _;
        let mut h = blake2::Blake2b::<blake2::digest::consts::U32>::new();
        for row in &rows {
            let cells: Vec<String> = row.0.iter().map(nils_ask::exec::render).collect();
            h.update(cells.join("\t").as_bytes());
            h.update(b"\n");
        }
        hex::encode(h.finalize())
    };
    let answer = nils_ask::exec::Answer {
        columns: vec!["_key".to_string(), "_subject".to_string()],
        rows,
        truncated: false,
        content_hash: Some(hash),
        elapsed_ms: 0,
        shaped: Vec::new(),
        held_back: Vec::new(),
        held_counts: Vec::new(),
        rows_held: false,
    };
    let node = nils_registry::job::hostname();
    let spec = nils_ask::handle::Spec {
        name: None,
        grain: nils_ask::Grain::Stack,
        ask: None,
        params: json!({}),
        selection_versions: match &pinned.versions {
            Value::Null => json!([]),
            v => v.clone(),
        },
        values_unresolved: json!({}),
        provenance: nils_ask::handle::Provenance {
            principal,
            node: &node,
            pack_version: pinned.pack_version,
            epoch,
            scheme_digest: pinned.scheme_digest,
            disclosure: "local",
            suppression: json!({"classes": ["technical"], "sealed": "read", "quasi": "shape"}),
        },
        page_rows: nils_catalog::Caps::default().page_rows as usize,
    };
    store.begin().map_err(|e| e.to_string())?;
    match nils_ask::handle::save(store, &spec, &answer) {
        Ok(h) => {
            store.commit().map_err(|e| e.to_string())?;
            Ok((h.id, answer.rows.len()))
        }
        Err(e) => {
            store.rollback().ok();
            Err(e.to_string())
        }
    }
}

/// The lock one step's door over one scope holds from its busy check to
/// its insert (#455), one per scope and step while a request holds or
/// waits for it: the last to let it go drops it, so the table holds the
/// turns being taken and no more.
struct DoorTurn {
    key: String,
    lock: std::sync::Arc<std::sync::Mutex<()>>,
}

type Turns =
    std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<std::sync::Mutex<()>>>>;

fn turns() -> &'static Turns {
    static TURNS: std::sync::OnceLock<Turns> = std::sync::OnceLock::new();
    TURNS.get_or_init(Default::default)
}

impl DoorTurn {
    fn take(target: &str, step: &str) -> DoorTurn {
        let key = format!("{target}\u{0}{step}");
        let mut all = turns().lock().unwrap_or_else(|e| e.into_inner());
        let lock = all.entry(key.clone()).or_default().clone();
        DoorTurn { key, lock }
    }

    /// Whether the table holds a turn for this scope and step, for the
    /// tests.
    #[cfg(test)]
    fn held(target: &str, step: &str) -> bool {
        turns()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&format!("{target}\u{0}{step}"))
    }
}

impl Drop for DoorTurn {
    fn drop(&mut self) {
        let mut all = turns().lock().unwrap_or_else(|e| e.into_inner());
        // the table's own reference and this one: nobody else holds or
        // waits for it
        if std::sync::Arc::strong_count(&self.lock) <= 2 {
            all.remove(&self.key);
        }
    }
}

/// Delete a handle this door froze and nothing reads: its members, its
/// pages and its row.
fn drop_handle(store: &mut Store, handle: i64) -> Result<(), StoreError> {
    for (table, column) in [
        ("handle_member", "handle_id"),
        ("handle_page", "handle_id"),
        ("handle", "id"),
    ] {
        let sql = format!(
            "DELETE FROM {} WHERE {column} = {}",
            store.qualified(table),
            store.dialect().param(1, nils_registry::schema::Type::Int)
        );
        store.execute(&sql, &[nils_registry::store::Param::Int(handle)])?;
    }
    Ok(())
}

/// The oldest job a door queued for the step over the scope, still open,
/// queued before `mine`: none where `mine` is the first.
fn earlier_open(
    store: &mut Store,
    step: &str,
    target: &str,
    mine: i64,
) -> Result<Option<i64>, StoreError> {
    Ok(asked(store, step, target)?
        .into_iter()
        .filter(|j| j.id < mine && j.open())
        .map(|j| j.id)
        .min())
}

/// Remove a job this door queued, while no worker has taken it: whether it
/// went (a job taken already runs and is answered as queued).
fn withdraw_queued(store: &mut Store, id: i64) -> Result<bool, StoreError> {
    let sql = format!(
        "DELETE FROM {} WHERE id = {} AND state = 'queued'",
        store.qualified("job"),
        store.dialect().param(1, nils_registry::schema::Type::Int)
    );
    Ok(store.execute(&sql, &[nils_registry::store::Param::Int(id)])? > 0)
}

/// The parameters a pipeline declares for a run over new stacks
/// (`x-nils.on-arrival`), as `id=value` pairs; none where it declares none
/// or is not in the catalog.
fn arrival_params(registry: &mut Registry, label: &str) -> Vec<(String, String)> {
    nils_registry::pipeline::resolve(registry.store(), label)
        .ok()
        .flatten()
        .and_then(|p| nils_pipeline::descriptor::from_value(p.descriptor.clone()).ok())
        .and_then(|d| d.on_arrival)
        .unwrap_or_default()
}

/// Who asks for a step's run and with what (2026-11-05): the registry
/// home, the packs and the pack a sort reads, the principal the job is
/// queued under, and what it records of who asked (the grants and the
/// detail a door's caller holds, or the keyboard's at the command line).
pub(crate) struct Asker<'a> {
    pub(crate) home: &'a nils_registry::home::Home,
    pub(crate) pack_dir: Option<&'a std::path::Path>,
    pub(crate) ask_pack: &'a str,
    pub(crate) principal: &'a str,
    pub(crate) queued_by: Value,
}

impl<'a> Asker<'a> {
    /// A door's caller.
    pub(crate) fn of_door(doors: &'a Doors, caller: &'a Caller) -> Asker<'a> {
        Asker {
            home: &doors.home,
            pack_dir: doors.pack_dir.as_deref(),
            ask_pack: &doors.ask_pack,
            principal: &caller.principal,
            queued_by: crate::serve::queued_by(caller),
        }
    }
}

/// What a step's door runs over (2026-11-06): a dataset's or a cohort's
/// scans, or a saved selection's, frozen now.
pub(crate) enum Target {
    View(crate::viewer::Scope),
    Selection(Chosen),
}

/// A saved selection frozen for a step (2026-11-06): its name and version,
/// the handle its stacks were frozen into whole, as `nils run --select
/// selection:NAME@V` freezes it, and what that handle pins.
pub(crate) struct Chosen {
    pub(crate) name: String,
    pub(crate) version: u64,
    pub(crate) handle: i64,
    versions: Value,
    pack_version: Option<String>,
}

impl Target {
    /// The dataset, the cohort or the selection a door names: `kind` is
    /// the door's `datasets`, `cohorts` or `selections`, and a selection is
    /// `NAME` or `NAME@V`, frozen here.
    pub(crate) fn named(
        asker: &Asker<'_>,
        registry: &mut Registry,
        kind: &str,
        name: &str,
    ) -> Result<Target, Reply> {
        Ok(match kind {
            "cohorts" => Target::View(crate::viewer::Scope::cohort(registry, name)?),
            "selections" => {
                let (name, version) = selection_named(name)?;
                Target::Selection(chosen(asker, registry, name, version)?)
            }
            _ => Target::View(crate::viewer::Scope::dataset(registry, name)?),
        })
    }

    /// What the step's job is for, as its state and log find it:
    /// `dataset:NAME`, `cohort:NAME` or `selection:NAME@V`.
    pub(crate) fn words(&self) -> String {
        match self {
            Target::View(scope) => format!("{}:{}", scope.kind(), scope.name()),
            Target::Selection(c) => format!("selection:{}@{}", c.name, c.version),
        }
    }

    /// The scans a step over it counts, as the operations scope them.
    fn held<'s>(&self, ids: &'s mut String) -> Result<Scope<'s>, Reply> {
        match self {
            Target::View(crate::viewer::Scope::Dataset { sources, .. }) => {
                *ids = list(sources);
                Ok(Scope::Sources(ids))
            }
            Target::View(crate::viewer::Scope::Cohort(c)) => Ok(Scope::Cohort(c.id)),
            // a step door names a dataset, a cohort or a selection, never
            // the registry
            Target::View(crate::viewer::Scope::Registry { .. }) => Err(Reply::error(
                400,
                "a step runs over a dataset, a cohort or a selection",
            )),
            Target::Selection(c) => Ok(Scope::Handle(c.handle)),
        }
    }

    /// What the run's handle pins of where its stacks came from.
    fn pinned(&self) -> Pinned<'_> {
        match self {
            Target::View(_) => Pinned::default(),
            Target::Selection(c) => Pinned {
                versions: c.versions.clone(),
                pack_version: c.pack_version.as_deref(),
                scheme_digest: None,
            },
        }
    }

    /// Let go of what was frozen to read it: a selection's own handle,
    /// which nothing reads once a run's handle is frozen from it.
    pub(crate) fn release(&self, store: &mut Store) {
        if let Target::Selection(c) = self {
            let _ = drop_handle(store, c.handle);
        }
    }
}

/// A selection as a door or `nils step run` names it: `NAME`, or `NAME@V`
/// for one version of it; its current version where none is named.
pub(crate) fn selection_named(words: &str) -> Result<(&str, Option<u64>), Reply> {
    let words = words.trim().trim_start_matches("selection:");
    let (name, version) = match words.rsplit_once('@') {
        Some((name, v)) => (
            name,
            Some(v.trim().parse::<u64>().map_err(|_| {
                Reply::error(
                    400,
                    format!("{words}: a selection is NAME, or NAME@V for one of its versions"),
                )
            })?),
        ),
        None => (words, None),
    };
    if name.trim().is_empty() {
        return Err(Reply::error(400, "name the selection"));
    }
    Ok((name.trim(), version))
}

/// A saved selection frozen whole into a handle of its stacks (2026-11-06),
/// as a run over it freezes it (`crate::ask_cli::freeze_doc`): a selection
/// of subjects or of visits by their stacks, under the pack the sort reads.
fn chosen(
    asker: &Asker<'_>,
    registry: &mut Registry,
    name: &str,
    version: Option<u64>,
) -> Result<Chosen, Reply> {
    use nils_ask::ast::Grain;
    let saved = nils_ask::selection::get(registry.store(), name, version)
        .map_err(|e| Reply::error(500, e.to_string()))?
        .ok_or_else(|| {
            Reply::error(
                404,
                match version {
                    Some(v) => format!("no version {v} of a selection named {name}"),
                    None => format!("no selection named {name}"),
                },
            )
        })?;
    let grain = saved
        .ask
        .sets
        .get(&saved.ask.out.set)
        .map(|s| s.grain)
        .unwrap_or(Grain::Subject);
    let spec = format!("selection:{}@{}", saved.name, saved.version);
    let doc = crate::campaigns::freeze_document(&saved.name, saved.version, grain, Grain::Stack)
        .map_err(|_| {
            Reply::error(
                400,
                format!(
                    "{spec} is a {} set; a step runs over the stacks of subjects, visits or stacks",
                    grain.name()
                ),
            )
        })?;
    let dir = crate::pack_dir(asker.home, asker.pack_dir.map(std::path::Path::to_path_buf))
        .map_err(|e| Reply::error(409, e.message))?;
    let pack = crate::ask_cli::load_pack(&dir, asker.ask_pack)
        .map_err(|e| Reply::error(409, e.message))?;
    let f = crate::ask_cli::freeze_doc(registry, &pack, &doc, crate::ask_cli::Freeze::Whole, &spec)
        .map_err(|e| Reply::error(400, e.message))?;
    let h = nils_ask::handle::get(registry.store(), f.handle)
        .map_err(|e| Reply::error(500, e.to_string()))?
        .ok_or_else(|| Reply::error(500, format!("handle {} is gone", f.handle)))?;
    Ok(Chosen {
        name: saved.name,
        version: saved.version,
        handle: f.handle,
        versions: h.selection_versions,
        pack_version: h.pack_version,
    })
}

/// The sort over a dataset's, a cohort's or a selection's scans, from its
/// card (2026-11-05): the scans frozen into a handle and a classify job over
/// them alone, by the served pack, as a scoped re-sort at the command line;
/// answered with the job. With `without` (2026-11-06), only the scans no
/// sort has judged, those outside the rules left out. Refused while a sort
/// over the same scans is queued or runs, and where there is no scan to
/// sort.
fn sort_door(
    asker: &Asker<'_>,
    registry: &mut Registry,
    target: &Target,
    over: Over,
) -> Result<Value, Reply> {
    let failed = |e: StoreError| Reply::error(500, e.to_string());
    let named = target.words();
    let turn = DoorTurn::take(&named, "sorted");
    let _held = turn.lock.lock().unwrap_or_else(|e| e.into_inner());
    let epoch = registry.meta().epoch;
    let mut ids = String::new();
    let held = target.held(&mut ids)?;
    let store = registry.store();
    // a sort of these scans queued or running already is the one that runs
    let running = store
        .query(
            &format!(
                "SELECT id FROM {} WHERE kind = 'classify' AND name = {} \
                 AND state IN ('queued', 'running') ORDER BY id LIMIT 1",
                store.qualified("job"),
                store.dialect().param(1, nils_registry::schema::Type::Text)
            ),
            &[nils_registry::store::Param::from(named.as_str())],
        )
        .map_err(failed)?
        .first()
        .and_then(|r| r.int(0).ok());
    if let Some(job) = running {
        return Err(Refused::new(
            409,
            "running",
            format!("a sort runs already over these scans, as job {job}"),
        )
        .reply("sorted"));
    }
    let only = match over {
        Over::All => None,
        Over::Without => {
            let holds = held.holds(store);
            Some(unsorted_condition(store, &holds).map_err(failed)?)
        }
    };
    let (handle, frozen) = freeze(
        store,
        held,
        only.as_deref(),
        asker.principal,
        epoch,
        &target.pinned(),
    )
    .map_err(|e| Reply::error(500, e))?;
    if frozen == 0 {
        drop_handle(store, handle).map_err(failed)?;
        return Err(match over {
            Over::All => Refused::new(409, "no_scans", "there are no scans to sort here"),
            Over::Without => Refused::new(
                409,
                "none_without",
                "every scan here is sorted already, or outside the rules",
            ),
        }
        .reply("sorted"));
    }
    let mut command = vec![
        "classify".to_string(),
        "--pack".to_string(),
        asker.ask_pack.to_string(),
        "--scope".to_string(),
        format!("handle:{handle}"),
        "--name".to_string(),
        format!("{named} sort"),
    ];
    if let Some(d) = asker.pack_dir {
        command.extend(["--pack-dir".to_string(), d.display().to_string()]);
    }
    let mut extra = asker.queued_by.clone();
    extra["step"] = json!("sorted");
    extra["for"] = json!(named);
    extra["over"] = json!(over.name());
    let id = nils_registry::job::enqueue_with(
        store,
        &command,
        Some(&named),
        Some(asker.principal),
        extra,
    )
    .map_err(crate::serve::job_err)?;
    Ok(json!({
        "job": id, "state": "queued", "step": "sorted", "for": named,
        "scans": frozen, "over": over.name(), "parts": 1, "handle": handle,
        "command": command,
    }))
}

/// How many parts a run of `pipeline` (its label) over `stacks` stacks is
/// split into, one run each, one after another in one job: one where they
/// fit one container of it, none for no stacks (2026-10-17's parts, said
/// before a run starts since 2026-11-05).
pub(crate) fn parts_of(store: &mut Store, pipeline: &str, stacks: i64) -> i64 {
    parts(stacks, part_size(store, pipeline))
}

/// How many stacks one part of a run of `pipeline` (its label) holds, none
/// where its runs are never split ([`crate::pipelines::part_stacks`]).
fn part_size(store: &mut Store, pipeline: &str) -> Option<i64> {
    nils_registry::pipeline::resolve(store, pipeline)
        .ok()
        .flatten()
        .and_then(|p| nils_pipeline::descriptor::from_value(p.descriptor).ok())
        .and_then(|d| crate::pipelines::part_stacks(&d))
        .and_then(|n| i64::try_from(n).ok())
        .filter(|n| *n > 0)
}

/// `stacks` stacks in parts of `size`: one where they fit, none for none.
fn parts(stacks: i64, size: Option<i64>) -> i64 {
    match size {
        _ if stacks <= 0 => 0,
        Some(n) if stacks > n => (stacks + n - 1) / n,
        _ => 1,
    }
}

/// `POST /api/datasets/{name}/steps/{step}/run`, `POST
/// /api/cohorts/{name}/steps/{step}/run` and, since 2026-11-06, `POST
/// /api/selections/{name}/steps/{step}/run` (a saved selection, `NAME` or
/// `NAME@V`, frozen whole first): the step's run over the scans it counts,
/// frozen now into a handle, queued for the dataset, the cohort or the
/// selection as a pipeline job under the caller's principal, grants and
/// detail, and
/// answered 202 with the job. Refused, with its reason (`reason`), while a
/// run of it over those scans is queued or runs, where nothing here runs
/// it (no pipeline or no model for it, no container runtime, no working
/// place: post-contrast always, until a model for it is installed), and
/// where there are no scans to run it over. The body may say which of the
/// scans (2026-11-05): `{"scans": "without"}` runs body part or
/// post-contrast over its "N without" alone, and the sort over the scans
/// not sorted (2026-11-06), `all` (the default) over every scan it counts.
pub(crate) fn run_door(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    kind: &str,
    name: &str,
    step: &str,
    body: &str,
) -> Result<Reply, Reply> {
    let doc = crate::serve::json_body(body)?;
    let over = match &doc["scans"] {
        Value::Null => Over::All,
        Value::String(words) => Over::parse(words).map_err(|e| Reply::error(400, e))?,
        _ => {
            return Err(Reply::error(400, "scans is all or without, as a string"));
        }
    };
    let asker = Asker::of_door(doors, caller);
    queue(&asker, registry, kind, name, step, over).map(Reply::accepted)
}

/// A step's run over a dataset's (`kind` datasets), a cohort's or a saved
/// selection's scans (`selections`, `NAME` or `NAME@V`, 2026-11-06), queued
/// as its door and `nils step run` queue it: the job, what it is for, how
/// many scans it froze and over which (`over`), the parts it runs in, the
/// handle and the command.
pub(crate) fn queue(
    asker: &Asker<'_>,
    registry: &mut Registry,
    kind: &str,
    name: &str,
    step: &str,
    over: Over,
) -> Result<Value, Reply> {
    // record 55 D3: System 1 is a step a door runs too, owning no axis
    let system1: (&str, &[&str]) = (crate::system1::STEP, &[]);
    let operation = OPERATIONS
        .iter()
        .chain(std::iter::once(&system1))
        .find(|(n, _)| *n == step)
        .copied();
    if step != "sorted" && operation.is_none() {
        return Err(Refused::new(
            404,
            "no_step",
            format!(
                "{step} is not a step a door runs; sorted, body_part, post_contrast and system1 are"
            ),
        )
        .reply(step));
    }
    // only a step that owns an axis, or the sort, has scans without its answer
    if over == Over::Without && operation.is_some_and(|(_, axes)| axes.is_empty()) {
        return Err(Reply::error(
            400,
            format!(
                "{step} runs over every scan; scans: without is for sorted, body_part and post_contrast"
            ),
        ));
    }
    let target = Target::named(asker, registry, kind, name)?;
    let queued = match operation {
        // 2026-11-05 (Nima: a cohort's card runs the sort and the main scans
        // too): the sort over the scope's scans is a step this door runs, as
        // a classify job over them frozen into a handle; the main scans are
        // the pick run door's, with the cohort or the dataset named
        None => sort_door(asker, registry, &target, over),
        Some(operation) => queue_operation(asker, registry, &target, operation, over),
    };
    target.release(registry.store());
    queued
}

/// [`queue`] for an operation of its own, or System 1, over the target's
/// scans.
fn queue_operation(
    asker: &Asker<'_>,
    registry: &mut Registry,
    scope: &Target,
    (step, axes): (&str, &[&str]),
    over: Over,
) -> Result<Value, Reply> {
    let failed = |e: StoreError| Reply::error(500, e.to_string());
    let target = scope.words();
    // #455: the busy check and the insert are one step: a second request
    // for the same step over the same scope waits here until the first has
    // queued its job, then finds it and is refused
    let turn = DoorTurn::take(&target, step);
    let _held = turn.lock.lock().unwrap_or_else(|e| e.into_inner());
    let planned = plan(registry, step).map_err(failed)?;
    let epoch = registry.meta().epoch;
    let mut ids = String::new();
    let held = scope.held(&mut ids)?;
    let store = registry.store();
    let only = (over == Over::Without).then(|| without_condition(store, step));
    let scans = count(
        store,
        &format!(
            "SELECT COUNT(*) FROM {} x WHERE {}{}",
            store.qualified("stack"),
            held.holds(store),
            only.as_deref()
                .map(|c| format!(" AND {c}"))
                .unwrap_or_default()
        ),
    )
    .map_err(failed)?;
    let (pipelines, _) = proposing(store, axes).map_err(failed)?;
    let at = Attempts::of(store, step, &pipelines, held, &target).map_err(failed)?;
    // every scan here has its answer: said as such, not as a scope with no scans
    if at.busy().is_none() && planned.is_ok() && only.is_some() && scans == 0 {
        return Err(Refused::new(
            409,
            "none_without",
            format!("every scan here has its {} answer already", title(step)),
        )
        .reply(step));
    }
    if let Some(refused) = refusal(step, at.busy(), &planned, scans) {
        return Err(refused.reply(step));
    }
    let Ok(plan) = planned else {
        return Err(Reply::error(500, "a refused run was planned"));
    };
    let (handle, frozen) = freeze(
        store,
        held,
        only.as_deref(),
        asker.principal,
        epoch,
        &scope.pinned(),
    )
    .map_err(|e| Reply::error(500, e))?;
    let arrival = arrival_params(registry, &plan.pipeline);
    let mut command = vec![
        "run".to_string(),
        plan.pipeline,
        "--handle".to_string(),
        handle.to_string(),
    ];
    for m in &plan.models {
        command.extend(["--model".to_string(), m.to_string()]);
    }
    // 2026-11-01 (Nima's decision 4 of 2026-10-30): a person's run from the
    // step's door takes the parameters the pipeline declares for a run over
    // new stacks (`x-nils.on-arrival`), as `nils score --step` does, so the
    // first body-part run fills the slice cache and a later head or
    // calibration update re-scores from it, not on the cache-less fast path
    for (id, value) in arrival {
        command.extend(["--param".to_string(), format!("{id}={value}")]);
    }
    let command = crate::pipelines::located(asker.pack_dir, command)?;
    // 2026-10-12: refused by its kind where it lacks what it needs here
    // (its scans are counted above, and its units are a stack each)
    crate::requirements::at_the_door(asker.home, asker.pack_dir, registry, &command, false)?;
    let store = registry.store();
    // 2026-10-17: more scans than one container of the pipeline holds are
    // run in parts, each a run of its own, in this one job
    let parts = parts_of(
        store,
        &command[1],
        i64::try_from(frozen).unwrap_or(i64::MAX),
    );
    let command = crate::pipelines::in_parts(store, command, frozen);
    // the step and what it is for, which its state and the dataset's log
    // find it by, beside who asked and under what; and which of its scans
    let mut extra = asker.queued_by.clone();
    extra["step"] = json!(step);
    extra["for"] = json!(target);
    extra["over"] = json!(over.name());
    let id = nils_registry::job::enqueue_with(
        store,
        &command,
        Some(&target),
        Some(asker.principal),
        extra,
    )
    .map_err(crate::serve::job_err)?;
    // and where another process queued the same step meanwhile, the older
    // job is the one that runs: this one goes while it is still queued
    if let Some(winner) = earlier_open(store, step, &target, id).map_err(failed)?
        && withdraw_queued(store, id).map_err(failed)?
    {
        // the scans this door froze for it are nobody's now
        drop_handle(store, handle).map_err(failed)?;
        return Err(Refused::new(
            409,
            "running",
            format!(
                "{} runs already over these scans, as job {winner}",
                title(step)
            ),
        )
        .reply(step));
    }
    Ok(json!({
        "job": id, "state": "queued", "step": step, "for": target,
        "scans": frozen, "over": over.name(), "parts": parts, "handle": handle,
        "command": command,
    }))
}

/// `GET /api/selections/{name}/steps` (2026-11-06): what each step's run
/// over a saved selection's scans (`NAME` or `NAME@V`) would go over before
/// it starts, as a dataset's summary says it of its steps: the sort, with
/// its scans sorted and not, then body part and post-contrast, each with
/// `runs` (every scan, and those without its answer, with the parts it
/// takes), where a run of it over these scans is, and why one would be
/// refused now. The selection is frozen whole to be counted, as a run over
/// it freezes it, and that handle let go once counted. Counts only.
pub(crate) fn selection_steps_door(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    name: &str,
) -> Result<Reply, Reply> {
    let asker = Asker::of_door(doors, caller);
    let (name, version) = selection_named(name)?;
    let target = Target::Selection(chosen(&asker, registry, name, version)?);
    let doc = match &target {
        Target::Selection(c) => over_selection(registry, c, !crate::sealed::reads(&caller.access)),
        Target::View(_) => Ok(Value::Null),
    };
    target.release(registry.store());
    doc.map(Reply::ok)
        .map_err(|e| Reply::error(500, e.to_string()))
}

/// A frozen selection's steps, as [`selection_steps_door`] answers them.
fn over_selection(
    registry: &mut Registry,
    c: &Chosen,
    hide_sealed: bool,
) -> Result<Value, StoreError> {
    let plans = plans(registry)?;
    let store = registry.store();
    let scope = Scope::Handle(c.handle);
    let holds = shown(store, scope.holds(store), hide_sealed);
    let (stack, class) = (store.qualified("stack"), store.qualified("classification"));
    let scans = count(
        store,
        &format!("SELECT COUNT(*) FROM {stack} x WHERE {holds}"),
    )?;
    let sorted = count(
        store,
        &format!(
            "SELECT COUNT(*) FROM {stack} x WHERE {holds} \
             AND EXISTS (SELECT 1 FROM {class} cl WHERE cl.stack_id = x.id)"
        ),
    )?;
    let (outside, outside_by) = crate::certainty::outside(store, &holds)?;
    let unsorted = (scans - sorted - outside).max(0);
    let named = format!("selection:{}@{}", c.name, c.version);
    // a sort of these scans queued or running, as the sort's door finds it
    let sorting = store
        .query(
            &format!(
                "SELECT id, state FROM {} WHERE kind = 'classify' AND name = {} \
                 AND state IN ('queued', 'running') ORDER BY id LIMIT 1",
                store.qualified("job"),
                store.dialect().param(1, nils_registry::schema::Type::Text)
            ),
            &[nils_registry::store::Param::from(named.as_str())],
        )?
        .first()
        .map(|r| Ok::<_, StoreError>((r.int(0)?, r.text(1)?.to_string())))
        .transpose()?;
    let refused = match &sorting {
        Some((job, _)) => Some(Refused::new(
            409,
            "running",
            format!("a sort runs already over these scans, as job {job}"),
        )),
        None if scans <= 0 => Some(Refused::new(
            409,
            "no_scans",
            "there are no scans to run it over",
        )),
        None => None,
    };
    let state = match &sorting {
        Some((_, state)) => state.as_str(),
        None if scans > 0 && sorted + outside >= scans => "done",
        None => "waiting",
    };
    let mut out = vec![json!({
        "step": "sorted",
        "state": state,
        "job": sorting.as_ref().map(|(job, _)| *job),
        "scans": sorted,
        "of": scans,
        "unsorted": unsorted,
        "outside": outside,
        "outside_by": outside_by,
        "refusal": refused.map(|r| r.doc()),
        "runs": sort_runs(scans, unsorted),
    })];
    out.extend(steps(store, scope, &named, scans, &plans, hide_sealed)?);
    Ok(json!({
        "selection": {"name": c.name, "version": c.version},
        "for": named,
        "scans": scans,
        "steps": out,
    }))
}

/// `nils step`: a dataset's or a cohort's steps from the command line, as
/// their doors run them.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum StepCommand {
    /// Queue a step's run over a dataset's, a cohort's or a saved
    /// selection's scans, as the step's Run on its card and its door (`POST
    /// /api/datasets/{name}/steps/{step}/run`) queue it: sorted, body_part,
    /// post_contrast or system1
    Run(StepRunArgs),
}

/// The arguments of `nils step run`.
#[derive(Debug, clap::Args)]
pub(crate) struct StepRunArgs {
    /// The dataset as @NAME or dataset:NAME, the cohort as cohort:NAME, or
    /// a saved selection as selection:NAME, or selection:NAME@V for one of
    /// its versions
    #[arg(value_name = "@NAME|dataset:NAME|cohort:NAME|selection:NAME[@V]")]
    scope: String,
    /// The step: sorted, body_part, post_contrast or system1
    step: String,
    /// Which of its scans: all, or without (the scans without its answer,
    /// the "N without" its step counts: no body part, T1 brain whose
    /// contrast is unknown, or not sorted; not for system1)
    #[arg(long, default_value = "all", value_name = "all|without")]
    scans: String,
    /// The pack a sort reads
    #[arg(long, default_value = "mri")]
    pack: String,
    /// Where the packs are, for the run the step queues
    #[arg(long, value_name = "DIR")]
    pack_dir: Option<std::path::PathBuf>,
}

/// The dataset's, the cohort's or the selection's name a `nils step run`
/// scope names, with the door's kind (`datasets`, `cohorts` or
/// `selections`).
pub(crate) fn scope_named(words: &str) -> Result<(&'static str, &str), String> {
    let words = words.trim();
    let (kind, name) = if let Some(name) = words.strip_prefix('@') {
        ("datasets", name)
    } else if let Some(name) = words.strip_prefix("dataset:") {
        ("datasets", name)
    } else if let Some(name) = words.strip_prefix("cohort:") {
        ("cohorts", name)
    } else if let Some(name) = words.strip_prefix("selection:") {
        ("selections", name)
    } else {
        return Err(format!(
            "{words}: a step runs over a dataset, @NAME or dataset:NAME, a cohort, cohort:NAME, or a saved selection, selection:NAME[@V]"
        ));
    };
    if name.trim().is_empty() {
        return Err(format!(
            "{words}: name the dataset, the cohort or the selection"
        ));
    }
    Ok((kind, name.trim()))
}

/// `nils step run`: the step's run queued as its door queues it, at the
/// keyboard's reach and under its principal; prints what the door answers.
pub(crate) fn command(
    home: &nils_registry::home::Home,
    cmd: StepCommand,
) -> Result<(), crate::Exit> {
    let StepCommand::Run(args) = cmd;
    let (kind, name) = scope_named(&args.scope).map_err(crate::usage)?;
    let over = Over::parse(&args.scans).map_err(crate::usage)?;
    let mut registry = crate::open(home)?;
    let keyboard = crate::grants::Access::keyboard(crate::sealed::keyboard_reads());
    let principal = crate::actor();
    let asker = Asker {
        home,
        pack_dir: args.pack_dir.as_deref(),
        ask_pack: &args.pack,
        principal: &principal,
        queued_by: json!({
            "detail": keyboard.detail.name(),
            "actor": principal,
            "grants": keyboard.list(),
        }),
    };
    match queue(&asker, &mut registry, kind, name, &args.step, over) {
        Ok(doc) => {
            println!("{doc}");
            Ok(())
        }
        Err(reply) => Err(crate::fail(
            reply.body["error"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| reply.body.to_string()),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{Plan, Refused, owner, refusal};

    /// A run withdrawn as the younger of two takes the scans it froze with
    /// it: its handle, members and pages are gone.
    #[test]
    fn a_withdrawn_run_s_handle_goes_with_it() {
        use nils_registry::store::{Cell, Row, Store};
        let mut store = Store::sqlite_in_memory().unwrap();
        nils_registry::migrate::migrate(&mut store, nils_registry::migrate::Kind::Registry)
            .unwrap();
        let rows = vec![
            Row(vec![Cell::Int(1), Cell::Int(10)]),
            Row(vec![Cell::Int(2), Cell::Int(10)]),
        ];
        let (handle, n) =
            super::save_stacks(&mut store, rows, "ops@lab", 1, &super::Pinned::default()).unwrap();
        assert_eq!(n, 2);
        let count =
            |store: &mut Store, sql: &str| store.query(sql, &[]).unwrap()[0].int(0).unwrap();
        assert_eq!(
            count(
                &mut store,
                &format!("SELECT COUNT(*) FROM handle WHERE id = {handle}")
            ),
            1
        );
        super::drop_handle(&mut store, handle).unwrap();
        for table in ["handle_member", "handle_page"] {
            assert_eq!(
                count(
                    &mut store,
                    &format!("SELECT COUNT(*) FROM {table} WHERE handle_id = {handle}")
                ),
                0,
                "{table}"
            );
        }
        assert_eq!(
            count(
                &mut store,
                &format!("SELECT COUNT(*) FROM handle WHERE id = {handle}")
            ),
            0
        );
    }

    /// #455's lock per scope and step is held while a request takes or
    /// waits for it, and the last to let it go drops it from the table.
    #[test]
    fn a_door_s_turn_is_dropped_once_nobody_takes_it() {
        use super::DoorTurn;
        let first = DoorTurn::take("dataset:turns", "body_part");
        let second = DoorTurn::take("dataset:turns", "body_part");
        assert!(std::sync::Arc::ptr_eq(&first.lock, &second.lock));
        drop(first);
        assert!(
            DoorTurn::held("dataset:turns", "body_part"),
            "one still holds it"
        );
        drop(second);
        assert!(!DoorTurn::held("dataset:turns", "body_part"));
    }

    /// 2026-11-06: a selection a step runs over is NAME, or NAME@V for one
    /// of its versions, with or without its `selection:` prefix.
    #[test]
    fn a_selection_is_named_with_its_version_or_without() {
        assert_eq!(super::selection_named("t1").ok(), Some(("t1", None)));
        assert_eq!(super::selection_named("t1@3").ok(), Some(("t1", Some(3))));
        assert_eq!(
            super::selection_named("selection:t1@2").ok(),
            Some(("t1", Some(2)))
        );
        for bad in ["", "@2", "t1@two", "selection:"] {
            assert!(super::selection_named(bad).is_err(), "{bad}");
        }
    }

    /// 2026-11-06: what a sort would go over, every scan or the ones not
    /// sorted, in one part each.
    #[test]
    fn a_sort_s_runs_say_every_scan_and_those_not_sorted() {
        use serde_json::json;
        assert_eq!(
            super::sort_runs(456_132, 135),
            json!({"all": {"scans": 456_132, "parts": 1}, "without": {"scans": 135, "parts": 1}})
        );
        assert_eq!(
            super::sort_runs(3, 0)["without"],
            json!({"scans": 0, "parts": 0})
        );
    }

    #[test]
    fn an_operation_owns_its_axes() {
        assert_eq!(owner("body_part"), Some("body_part"));
        assert_eq!(owner("body_region"), Some("body_part"));
        assert_eq!(owner("post_contrast"), Some("post_contrast"));
        assert_eq!(owner("technique"), None);
    }

    #[test]
    fn a_run_is_refused_while_one_goes_on_then_by_its_plan_then_for_no_scans() {
        let plan = Ok(Plan {
            pipeline: "bodypart-infer-fusion@1".to_string(),
            models: vec![1, 2],
        });
        let no_model = Err(Refused {
            status: 409,
            reason: "no_model",
            error: "no post-contrast model is installed".to_string(),
        });
        let running = refusal("body_part", Some(Some(7)), &plan, 3).unwrap();
        assert_eq!((running.status, running.reason), (409, "running"));
        assert!(running.error.contains("job 7"), "{}", running.error);
        assert!(
            running.error.starts_with("body part runs"),
            "{}",
            running.error
        );
        let refused = refusal("post_contrast", None, &no_model, 3).unwrap();
        assert_eq!(refused.reason, "no_model");
        assert_eq!(refused.error, "no post-contrast model is installed");
        assert_eq!(
            refusal("body_part", None, &plan, 0).map(|r| r.reason),
            Some("no_scans")
        );
        assert_eq!(refusal("body_part", None, &plan, 3), None);
    }

    /// 2026-11-05: a going run's step reads its run's own beat over the unit
    /// rows' count: a container's own count, its part and since when, never
    /// the lane's figures, and the rows' count where the beat has none.
    #[test]
    fn a_step_s_progress_is_its_run_s_beat_over_its_rows() {
        use serde_json::{Value, json};
        let rows = json!({"done": 0, "total": 870});
        let beat = json!({"run": 3, "phase": "run", "units": 870, "over": 0, "done": 312,
            "total": 870, "what": "stacks", "part": {"at": 4, "of": 23},
            "since": "2026-11-05T22:00:00Z", "lane": {"cores": 64}});
        let p = super::with_live(rows.clone(), beat, true).unwrap();
        assert_eq!((&p["done"], &p["total"]), (&json!(312), &json!(870)));
        assert_eq!(p["part"], json!({"at": 4, "of": 23}));
        assert!(p.get("lane").is_none(), "{p}");
        // a beat as the input is prepared says its phase and keeps the rows' count
        let p = super::with_live(
            rows.clone(),
            json!({"run": 3, "phase": "materialise", "done": null}),
            true,
        )
        .unwrap();
        assert_eq!(
            (&p["done"], &p["phase"]),
            (&json!(0), &json!("materialise"))
        );
        // no beat: the rows' count where there are rows, else nothing
        assert_eq!(
            super::with_live(rows.clone(), Value::Null, true),
            Some(rows.clone())
        );
        assert_eq!(super::with_live(rows, Value::Null, false), None);
    }
}
