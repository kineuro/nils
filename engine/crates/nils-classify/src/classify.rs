// SPDX-License-Identifier: AGPL-3.0-only

//! `nils classify` (`docs/specs/wave2-fingerprint-and-classify.md`, §8, §9).
//!
//! A job of the same shape as the fingerprint: windows of stacks, one
//! transaction each, resumable. What it writes that v0 does not is the
//! **evidence**: v0 computes the tier, the confidence and the matched token
//! and then throws all three away, so nothing about a classified stack
//! explains itself. Here the evidence is the verdict's other half.
//!
//! And a **decision outranks a rule**. The rule's answer is computed as
//! usual, the decision wins, the difference is recorded, and a review item is
//! raised only when the two disagree (C15).

use std::collections::HashMap;
use std::time::Instant;

use nils_pack::stack::Value as PackValue;
use nils_pack::{Evaluated, Pack, Stack};
use nils_registry::dialect::Conflict;
use nils_registry::schema::{Type, table};
use nils_registry::store::{Insert, Param, Row, Store};
use nils_registry::{Registry, time::now_iso};

use crate::job::Error;

/// How many stacks a window holds.
pub const WINDOW: usize = 4_096;

/// The fingerprint columns a pack is fed, in the order the select reads them.
/// The pack names a field; this is where a name becomes a column.
/// The field a pack names, and the fingerprint column it is read from.
///
/// **Index-aligned with [`nils_pack::stack::FIELDS`].** A pass turns a field
/// index from that list into a column with this one, so a field added to one
/// and not the other reads a neighbouring column and says nothing about it.
/// `fields_line_up` is what catches that.
pub(crate) const FIELDS: &[(&str, &str)] = &[
    ("echo_time", "echo_time"),
    ("repetition_time", "repetition_time"),
    ("inversion_time", "inversion_time"),
    ("flip_angle", "flip_angle"),
    ("echo_train_length", "echo_train_length"),
    ("magnetic_field_strength", "magnetic_field_strength"),
    ("slice_thickness", "slice_thickness"),
    ("spacing_between_slices", "spacing_between_slices"),
    ("number_of_averages", "number_of_averages"),
    ("n_instances", "n_instances"),
    ("stacks_in_series", "stacks_in_series"),
    ("orientation_confidence", "orientation_confidence"),
    ("rows", "rows"),
    ("columns", "columns"),
    ("fov_x", "fov_x"),
    ("fov_y", "fov_y"),
    ("aspect_ratio", "aspect_ratio"),
    ("field_strength_normalized", "field_strength_normalized"),
    ("dwi_directions", "dwi_directions"),
    ("modality", "modality"),
    ("manufacturer", "manufacturer"),
    ("manufacturer_model_name", "manufacturer_model_name"),
    ("station_name", "station_name"),
    ("implementation_class_uid", "implementation_class_uid"),
    ("implementation_version_name", "implementation_version_name"),
    ("mr_acquisition_type", "mr_acquisition_type"),
    ("orientation", "orientation"),
    ("split_reason", "split_reason"),
    ("echo_numbers", "echo_numbers"),
    ("diffusion_b_value", "diffusion_b_value"),
    ("pixel_bandwidth", "pixel_bandwidth"),
    ("pixel_spacing", "pixel_spacing"),
    ("image_type", "image_type"),
    ("scanning_sequence", "scanning_sequence"),
    ("sequence_variant", "sequence_variant"),
    ("scan_options", "scan_options"),
    ("image_orientation_patient", "image_orientation_patient"),
    ("text_series_description", "text_series_description"),
    ("text_protocol_name", "text_protocol_name"),
    ("text_sequence_name", "text_sequence_name"),
    ("text_body_part", "text_body_part"),
    ("text_series_comments", "text_series_comments"),
    ("text_image_comments", "text_image_comments"),
    ("text_all", "text_all"),
    ("text_contrast", "text_contrast"),
    ("image_role", "image_role"),
    ("acquisition_type_filled", "acquisition_type_filled"),
    ("acquisition_type_source", "acquisition_type_source"),
    ("dwi_b_values", "dwi_b_values"),
    ("dwi_b_value_source", "dwi_b_value_source"),
    ("dwi_pe_direction", "dwi_pe_direction"),
    ("dwi_pe_direction_source", "dwi_pe_direction_source"),
    ("dwi_directions_source", "dwi_directions_source"),
    ("field_strength_unit", "field_strength_unit"),
];

/// The select that reads one window of fingerprints, ordered by stack. With
/// `ids`, it also says which series and subject each stack belongs to, which
/// is what a decision wider than a stack is matched on.
fn select(store: &Store, modality: Option<&str>, ids: bool) -> String {
    let t = table("stack_fingerprint");
    let dialect = store.dialect();
    let head = if ids {
        vec![
            "f.stack_id".to_string(),
            "k.series_id".to_string(),
            "r.subject_id".to_string(),
        ]
    } else {
        vec!["f.stack_id".to_string()]
    };
    let cols: Vec<String> = head
        .into_iter()
        .chain(FIELDS.iter().map(|(_, c)| {
            let column = t
                .column(c)
                .unwrap_or_else(|| panic!("stack_fingerprint.{c} is not a column"));
            dialect.text_of_qualified(Some("f"), column)
        }))
        .collect();
    let joins = if ids {
        format!(
            " JOIN {} AS k ON k.id = f.stack_id JOIN {} AS r ON r.id = k.series_id",
            store.qualified("stack"),
            store.qualified("series"),
        )
    } else {
        String::new()
    };
    let filter = match modality {
        Some(m) => format!(" AND f.modality = '{}'", m.replace('\'', "''")),
        None => String::new(),
    };
    format!(
        "SELECT {} FROM {} AS f{joins} WHERE f.stack_id > {}{filter} ORDER BY f.stack_id LIMIT {}",
        cols.join(", "),
        store.qualified("stack_fingerprint"),
        dialect.param(1, Type::Int),
        dialect.param(2, Type::Int),
    )
}

/// A cell as the text a pack reads. The fingerprint's columns are typed and a
/// pack's fields are named, so this is where a double becomes the string a
/// substring test can look at and a number can still be parsed back.
pub(crate) fn cell_text(c: &nils_registry::store::Cell) -> Option<String> {
    use nils_registry::store::Cell;
    match c {
        Cell::Null => None,
        Cell::Text(t) => Some(t.clone()),
        Cell::Int(i) => Some(i.to_string()),
        Cell::Double(d) => Some(format!("{d}")),
        Cell::Bool(b) => Some(b.to_string()),
        Cell::Bytes(_) => None,
    }
}

/// One fingerprint row as the pack sees it.
fn to_stack(r: &Row, with_ids: bool) -> Result<(Ids, Stack), Error> {
    let first = if with_ids { 3 } else { 1 };
    let mut s = Stack::new();
    for (i, (field, _)) in FIELDS.iter().enumerate() {
        let v = cell_text(r.get(i + first));
        s.set(field, PackValue::Text(v.as_deref()))
            .expect("a field the pack declares");
    }
    let ids = Ids {
        stack: r.int(0)?,
        series: if with_ids { r.int(1)? } else { 0 },
        subject: if with_ids { r.int(2)? } else { 0 },
    };
    Ok((ids, s))
}

/// The decisions in force, by scope and axis. A stack's own decision wins over
/// its series', and so on outward.
///
/// A decision is recorded at one of four scopes (§8.3): this stack, this
/// series, this subject, or an origin (`manufacturer=SIEMENS`, `model=...`,
/// `station=...`). The narrowest one that names a stack is the one that
/// applies, so a call about one series does not quietly govern the site and a
/// call about the site is overridden where somebody looked closer.
pub struct Decisions {
    by_stack: HashMap<(i64, String), Decided>,
    by_series: HashMap<(i64, String), Decided>,
    by_subject: HashMap<(i64, String), Decided>,
    /// Keyed by the lowercase `field=value` the decision names.
    by_origin: HashMap<(String, String), Decided>,
}

/// What a decision says, and who said it (§10.1).
#[derive(Debug, Clone)]
pub struct Decided {
    /// The value, or none when the decision is that the axis has none.
    pub value: Option<String>,
    /// How far the call reached: `stack`, `series`, `subject` or `origin`.
    pub scope: String,
    pub author: Author,
}

/// Who made a decision.
///
/// v0 has no such thing, and its worst outcome came from the absence: 4,692
/// body parts in the live archive are an image model's predictions, committed
/// by a person through its body-part QC straight into the classifier's own
/// column, with nothing to mark them. They are discoverable only because v0's
/// keyword classifier disagrees, answering nothing for 4,692 of that cohort's
/// 4,699 stacks. A value a model produced may not sit where a rule's answer
/// belongs and look the same.
#[derive(Debug, Clone)]
pub struct Author {
    /// The name, the account or the model's registered id.
    pub who: String,
    /// `person`, `agent` or `model`.
    pub kind: String,
    /// A model's version. Null for anything else (D15).
    pub version: Option<String>,
}

/// Which stack a fingerprint row belongs to, and to what. Read only when a
/// decision wider than a stack exists, so the usual run is one table.
#[derive(Clone, Copy, Default)]
struct Ids {
    stack: i64,
    series: i64,
    subject: i64,
}

/// The fields that name an origin, and the fingerprint field each one
/// reads. The word is deliberately not `provenance`: the MRI pack has an
/// axis of that name meaning how an image was produced, and the engine must
/// not borrow a pack's vocabulary for one of its own ideas.
/// field each one reads.
const ORIGIN: &[(&str, &str)] = &[
    ("manufacturer", "manufacturer"),
    ("model", "manufacturer_model_name"),
    ("station", "station_name"),
];

impl Decisions {
    fn load(store: &mut Store) -> Result<Decisions, Error> {
        let sql = format!(
            "SELECT scope, ref, axis, value, actor, author_kind, author_version FROM {} \
             WHERE withdrawn_at IS NULL",
            store.qualified("decision")
        );
        let mut d = Decisions {
            by_stack: HashMap::new(),
            by_series: HashMap::new(),
            by_subject: HashMap::new(),
            by_origin: HashMap::new(),
        };
        for r in store.query(&sql, &[])? {
            let scope = r.text(0)?;
            let reference = r.text(1)?.to_string();
            let axis = r.text(2)?.to_string();
            let author = Author {
                who: r.text(4)?.to_string(),
                kind: r.opt_text(5)?.unwrap_or("person").to_string(),
                version: r.opt_text(6)?.map(str::to_string),
            };
            let value = Decided {
                value: r.opt_text(3)?.map(str::to_string),
                scope: scope.to_string(),
                author,
            };
            let id = || reference.parse::<i64>().unwrap_or(0);
            match scope {
                "stack" => {
                    d.by_stack.insert((id(), axis), value);
                }
                "series" => {
                    d.by_series.insert((id(), axis), value);
                }
                "subject" => {
                    d.by_subject.insert((id(), axis), value);
                }
                "origin" => {
                    d.by_origin.insert((reference.to_lowercase(), axis), value);
                }
                // A scope the engine does not know is not silently obeyed.
                _ => {}
            }
        }
        Ok(d)
    }

    /// Whether any decision reaches past a single stack, which is what makes
    /// the run pay for the join that says which series and subject a stack is.
    fn needs_ids(&self) -> bool {
        !self.by_series.is_empty() || !self.by_subject.is_empty()
    }

    fn any(&self) -> bool {
        !self.by_stack.is_empty()
            || !self.by_series.is_empty()
            || !self.by_subject.is_empty()
            || !self.by_origin.is_empty()
    }

    /// The decision in force on this axis of this stack, narrowest first.
    fn for_stack(&self, ids: Ids, stack: &Stack, axis: &str) -> Option<&Decided> {
        let key = |id: i64| (id, axis.to_string());
        if let Some(v) = self.by_stack.get(&key(ids.stack)) {
            return Some(v);
        }
        if let Some(v) = self.by_series.get(&key(ids.series)) {
            return Some(v);
        }
        if let Some(v) = self.by_subject.get(&key(ids.subject)) {
            return Some(v);
        }
        if self.by_origin.is_empty() {
            return None;
        }
        for (name, field) in ORIGIN {
            let Some(index) = nils_pack::stack::field_index(field) else {
                continue;
            };
            let value = stack.text(index);
            if value.is_empty() {
                continue;
            }
            let reference = format!("{name}={}", value.to_lowercase());
            if let Some(v) = self.by_origin.get(&(reference, axis.to_string())) {
                return Some(v);
            }
        }
        None
    }
}

/// Classify every stack in scope.
pub fn classify(
    registry: &mut Registry,
    pack: &Pack,
    settings: &crate::job::Settings,
    cancel: &nils_digest::Cancel,
) -> Result<crate::report::Classified, Error> {
    let started = Instant::now();
    let job_id = crate::job::claim_for(registry, settings, "classify")?;
    let result = run(registry, pack, settings, cancel, job_id, started);
    let store = registry.store();
    match &result {
        Ok(report) => {
            let state = if report.cancelled {
                "cancelled"
            } else {
                "done"
            };
            crate::job::finish(store, job_id, state, None)?;
        }
        Err(e) => {
            let text = e.to_string();
            crate::job::finish(store, job_id, "failed", Some(&text)).ok();
        }
    }
    result
}

fn run(
    registry: &mut Registry,
    pack: &Pack,
    settings: &crate::job::Settings,
    cancel: &nils_digest::Cancel,
    job_id: i64,
    started: Instant,
) -> Result<crate::report::Classified, Error> {
    let epoch = registry.meta().epoch;
    let mut report = crate::report::Classified::new(job_id, epoch, pack.id());
    let window = settings.window.max(1);
    let store = registry.store();
    let decisions = Decisions::load(store)?;
    let with_ids = decisions.needs_ids();
    // Nothing was decided by a person, so nothing is looked up per axis.
    let any_decision = decisions.any();
    let sql = select(store, settings.modality.as_deref(), with_ids);
    let now = now_iso();

    let class_t = table("classification");
    let axis_t = table("classification_axis");
    let ev_t = table("classification_evidence");
    let review_t = table("review_item");

    // Whether any classifier question is still open from an earlier run. A
    // first run has none, and then no window pays for the check.
    let stale = store
        .query(
            &format!(
                "SELECT COUNT(*) FROM {} WHERE status = 'open' AND kind LIKE '%:%'",
                store.qualified("review_item")
            ),
            &[],
        )?
        .first()
        .and_then(|r| r.int(0).ok())
        .unwrap_or(0)
        > 0;

    let mut after: i64 = 0;
    loop {
        if cancel.stop() {
            report.cancelled = true;
            break;
        }
        let rows = store.query(&sql, &[Param::Int(after), Param::Int(window as i64)])?;
        if rows.is_empty() {
            break;
        }
        let last = rows.last().expect("a non-empty window").int(0)?;

        let mut classes: Vec<Vec<Param>> = Vec::with_capacity(rows.len());
        let mut axes: Vec<Vec<Param>> = Vec::new();
        let mut evidence: Vec<Vec<Param>> = Vec::new();
        let mut reviews: Vec<Vec<Param>> = Vec::new();

        for r in &rows {
            let (ids, stack) = to_stack(r, with_ids)?;
            let stack_id = ids.stack;
            // The decisions that overrode a rule for this stack, with who made
            // each: written as evidence after the rules' own (§10.1).
            let mut authored: Vec<(String, String, Decided)> = Vec::new();
            report.read += 1;
            // A pack that does not judge this modality says so on the row: it
            // is never `misc` and never a review item, which is what v0 does
            // with every CT and PET stack it pushes through the MRI rules.
            let modality =
                stack.text(nils_pack::stack::field_index("modality").expect("modality is a field"));
            if modality != pack.modality {
                report.no_pack += 1;
                continue;
            }
            let verdict = Evaluated::new(pack, &stack).classify();
            let mut raised = 0i64;

            for a in &verdict.axes {
                let mut value = a.stored();
                let mut tier = a.tier.clone();
                if let Some(d) = any_decision
                    .then(|| decisions.for_stack(ids, &stack, &a.axis))
                    .flatten()
                {
                    let decided = d.value.clone().unwrap_or_default();
                    if decided != value {
                        // The rule's answer is computed as usual and the
                        // decision wins; the disagreement is the review item,
                        // not the decision.
                        reviews.push(vec![
                            Param::from(format!("{}:decision", a.axis)),
                            Param::from("stack"),
                            Param::from(serde_json::json!({"stack_id": stack_id}).to_string()),
                            Param::from(
                                serde_json::json!({
                                    "axis": a.axis,
                                    "rule": value,
                                    "decision": decided,
                                    "pack": pack.id(),
                                })
                                .to_string(),
                            ),
                            Param::from("open"),
                            Param::from(now.as_str()),
                        ]);
                        raised += 1;
                    }
                    value = decided;
                    tier = "decision".to_string();
                    report.decided += 1;
                    // §10.1. The verdict's own evidence says which rule
                    // reached which answer; this says who overrode it, and
                    // with what standing. Without it a model's prediction sits
                    // in the same column as a rule's answer and reads the
                    // same, which is v0's 4,692 body parts.
                    authored.push((a.axis.clone(), value.clone(), d.clone()));
                }
                // What decided each axis, counted: a rule that fires on a
                // sixth of the archive is a question about the rule, and the
                // count is how it is asked. Asking it once per stack is what
                // makes v0's queue unreadable.
                *report
                    .by_tier
                    .entry(format!("{}:{}", a.axis, tier))
                    .or_insert(0) += 1;
                axes.push(vec![
                    Param::Int(stack_id),
                    Param::from(a.axis.as_str()),
                    if value.is_empty() {
                        Param::Null
                    } else {
                        Param::from(value.as_str())
                    },
                    Param::Double(a.confidence),
                    Param::from(tier.as_str()),
                ]);
                // What a person is asked about, and where the number comes
                // from: the pack declares it per axis, because what counts as
                // a weak answer is knowledge about the domain and not about
                // databases. v0 flags 84 percent of its stacks, mostly for a
                // missing keyword rather than for doubt; the count here is in
                // the report so that a pack cannot quietly do the same.
                let below = settings.review_below.unwrap_or(pack.review.below(&a.axis));
                let missing = value.is_empty();
                let ask = !verdict.silent
                    && if missing {
                        pack.review.asks_when_missing(&a.axis)
                    } else {
                        a.confidence > 0.0 && a.confidence < below
                    };
                if ask {
                    let kind = if missing { "missing" } else { "low_confidence" };
                    reviews.push(vec![
                        Param::from(format!("{}:{kind}", a.axis)),
                        Param::from("stack"),
                        Param::from(serde_json::json!({"stack_id": stack_id}).to_string()),
                        Param::from(
                            serde_json::json!({
                                "axis": a.axis,
                                "value": a.stored(),
                                "confidence": a.confidence,
                                "tier": a.tier,
                                "below": below,
                            })
                            .to_string(),
                        ),
                        Param::from("open"),
                        Param::from(now.as_str()),
                    ]);
                    raised += 1;
                }
            }

            // A decision on an axis the rules said nothing about. Without
            // this it is silently dropped, because the loop above walks the
            // verdict and an axis with no hits and no default produces none.
            //
            // That gap is the whole of v0's 4,692 body parts: its keyword
            // classifier answers nothing for 4,692 of that cohort's 4,699
            // stacks, and the image model's predictions are what fill them.
            // An engine that cannot record an answer where the rules were
            // silent leaves a person no place to put one but the rules' own
            // column, which is how v0 came to have values nobody can trace.
            if any_decision {
                for a in &pack.axes {
                    if a.phase != nils_pack::rules::AxisPhase::Class
                        || verdict.axes.iter().any(|v| v.axis == a.name)
                    {
                        continue;
                    }
                    let Some(d) = decisions.for_stack(ids, &stack, &a.name) else {
                        continue;
                    };
                    let value = d.value.clone().unwrap_or_default();
                    report.decided += 1;
                    *report
                        .by_tier
                        .entry(format!("{}:decision", a.name))
                        .or_insert(0) += 1;
                    axes.push(vec![
                        Param::Int(stack_id),
                        Param::from(a.name.as_str()),
                        if value.is_empty() {
                            Param::Null
                        } else {
                            Param::from(value.as_str())
                        },
                        Param::Double(1.0),
                        Param::from("decision"),
                    ]);
                    authored.push((a.name.clone(), value, d.clone()));
                }
            }

            for e in &verdict.evidence {
                evidence.push(vec![
                    Param::Int(stack_id),
                    Param::from(e.axis.as_str()),
                    Param::from(e.value.as_str()),
                    Param::from(e.tier.as_str()),
                    Param::Double(e.confidence),
                    Param::from(e.rule_set.as_str()),
                    Param::from(e.rule.as_str()),
                    Param::from(e.source.as_str()),
                    if e.matched.is_empty() {
                        Param::Null
                    } else {
                        Param::from(e.matched.as_str())
                    },
                    Param::Null,
                    Param::Null,
                ]);
            }
            for (axis, value, d) in &authored {
                evidence.push(vec![
                    Param::Int(stack_id),
                    Param::from(axis.as_str()),
                    Param::from(value.as_str()),
                    Param::from("decision"),
                    Param::Double(1.0),
                    Param::from("decision"),
                    Param::from(d.scope.as_str()),
                    Param::from("decision"),
                    match &d.author.version {
                        Some(v) => Param::from(v.as_str()),
                        None => Param::Null,
                    },
                    Param::from(d.author.who.as_str()),
                    Param::from(d.author.kind.as_str()),
                ]);
            }
            report.evidence += (verdict.evidence.len() + authored.len()) as i64;
            report.silent += i64::from(verdict.silent);
            report.review_items += raised;
            report.written += 1;
            classes.push(vec![
                Param::Int(stack_id),
                Param::from(pack.name.as_str()),
                Param::from(pack.version.to_string()),
                Param::Int(i64::from(pack.contract)),
                match &pack.overlay {
                    Some(o) => Param::from(o.as_str()),
                    None => Param::Null,
                },
                Param::Int(job_id),
                Param::Int(epoch),
                Param::Int(raised),
            ]);
        }

        if !classes.is_empty() {
            store.begin()?;
            let write = (|| -> Result<(), nils_registry::store::Error> {
                // A re-classification replaces what it decided before, which
                // is why the row carries the pack version that decided it.
                let ids: Vec<i64> = classes
                    .iter()
                    .map(|c| match c[0] {
                        Param::Int(i) => i,
                        _ => 0,
                    })
                    .collect();
                // A question this run asks again is asked once, not twice:
                // the open items of the stacks being re-judged are marked
                // superseded before the new ones are written. What a person
                // already answered is accepted and stays that way.
                if stale {
                    for chunk in ids.chunks(256) {
                        let holes: Vec<String> = (0..chunk.len())
                            .map(|i| store.dialect().param(i + 1, Type::Json))
                            .collect();
                        let sql = format!(
                            "UPDATE {} SET status = 'superseded' WHERE status = 'open' AND scope = 'stack' AND kind LIKE '%:%' AND ref IN ({})",
                            store.qualified("review_item"),
                            holes.join(", "),
                        );
                        let params: Vec<Param> = chunk
                            .iter()
                            .map(|id| Param::from(serde_json::json!({"stack_id": id}).to_string()))
                            .collect();
                        store.execute(&sql, &params)?;
                    }
                }
                for t in [axis_t, ev_t] {
                    let sql = format!(
                        "DELETE FROM {} WHERE stack_id > {} AND stack_id <= {}",
                        store.qualified(t.name),
                        store.dialect().param(1, Type::Int),
                        store.dialect().param(2, Type::Int),
                    );
                    store.execute(
                        &sql,
                        &[
                            Param::Int(after),
                            Param::Int(*ids.iter().max().unwrap_or(&after)),
                        ],
                    )?;
                }
                let overwritten: Vec<&str> = vec![
                    "pack",
                    "pack_version",
                    "contract",
                    "overlay",
                    "job_id",
                    "epoch",
                    "review_items",
                ];
                store.insert(
                    &Insert::new(
                        class_t,
                        &[
                            "stack_id",
                            "pack",
                            "pack_version",
                            "contract",
                            "overlay",
                            "job_id",
                            "epoch",
                            "review_items",
                        ],
                    )
                    .on_conflict(Conflict::Update {
                        target: &["stack_id"],
                        set: &overwritten,
                    }),
                    &classes,
                )?;
                store.insert(
                    &Insert::new(axis_t, &["stack_id", "axis", "value", "confidence", "tier"]),
                    &axes,
                )?;
                store.insert(
                    &Insert::new(
                        ev_t,
                        &[
                            "stack_id",
                            "axis",
                            "value",
                            "tier",
                            "confidence",
                            "rule_set",
                            "rule",
                            "source",
                            "matched",
                            "author",
                            "author_kind",
                        ],
                    ),
                    &evidence,
                )?;
                if !reviews.is_empty() {
                    store.insert(
                        &Insert::new(
                            review_t,
                            &["kind", "scope", "ref", "evidence", "status", "created_at"],
                        ),
                        &reviews,
                    )?;
                }
                Ok(())
            })();
            match write {
                Ok(()) => store.commit()?,
                Err(e) => {
                    store.rollback().ok();
                    return Err(Error::Store(e));
                }
            }
        }

        crate::job::beat(store, job_id)?;
        after = last;
        if rows.len() < window {
            break;
        }
    }

    // The passes: the phases that read more than one stack. They run once,
    // after every stack has a verdict, against the reference the pack named.
    if !report.cancelled {
        report.passes = crate::passes::run(
            store,
            pack,
            settings,
            cancel,
            nils_pack::pass::Phase::After,
            job_id,
        )?;
        for p in &report.passes {
            report.review_items += p.review_items;
        }
    }

    // And last, what to do with each stack (Wave 3 §7), from what the rules
    // and the passes between them decided.
    if !report.cancelled {
        report.disposed = dispose(store, pack, settings, cancel, job_id, &sql)?;
    }

    report.seconds = started.elapsed().as_secs_f64();
    report.peak_rss = nils_digest::rss::peak_rss();
    Ok(report)
}

/// §7: the disposition of every stack, decided after the passes.
///
/// A third phase rather than another rule set, because it is worked out from
/// what was decided rather than from what was measured, and the passes fill
/// axes. A disposition settled in the rules phase would be settled from a gap:
/// v0 has exactly that fault, and patches it with a second copy of its intent
/// cascade inside gap filling, which then disagrees with the first.
///
/// Per stack and nothing else. §7's other structural rule is that a
/// disposition never depends on what else is in the selection, and here that
/// is not a check but the shape of the function: it is handed one stack's
/// fields and one stack's decided axes, and there is nothing else to read.
fn dispose(
    store: &mut Store,
    pack: &Pack,
    settings: &crate::job::Settings,
    cancel: &nils_digest::Cancel,
    job_id: i64,
    sql: &str,
) -> Result<i64, Error> {
    let names: Vec<&str> = pack
        .axes
        .iter()
        .filter(|a| a.phase == nils_pack::rules::AxisPhase::Disposition)
        .map(|a| a.name.as_str())
        .collect();
    if names.is_empty() {
        return Ok(0);
    }
    let window = settings.window.max(1);
    let axis_t = table("classification_axis");
    let ev_t = table("classification_evidence");
    let quoted: Vec<String> = names.iter().map(|n| format!("'{n}'")).collect();
    let scope = format!("axis IN ({})", quoted.join(", "));

    let mut disposed = 0i64;
    let mut after: i64 = 0;
    loop {
        if cancel.stop() {
            break;
        }
        let rows = store.query(sql, &[Param::Int(after), Param::Int(window as i64)])?;
        if rows.is_empty() {
            break;
        }
        let last = rows.last().expect("a non-empty window").int(0)?;

        // What the rules and the passes left, for the stacks of this window.
        // A multi-valued axis is stored comma-joined, so it is split back into
        // the values the rules put there.
        let mut decided: HashMap<i64, Vec<Vec<String>>> = HashMap::new();
        let seed_sql = format!(
            "SELECT stack_id, axis, value FROM {} WHERE stack_id > {} AND stack_id <= {}",
            store.qualified("classification_axis"),
            store.dialect().param(1, Type::Int),
            store.dialect().param(2, Type::Int),
        );
        for r in store.query(&seed_sql, &[Param::Int(after), Param::Int(last)])? {
            let name = r.text(1)?;
            let Some(a) = pack.axes.iter().position(|x| x.name == name) else {
                continue;
            };
            let value = r.opt_text(2)?.unwrap_or("");
            if value.is_empty() {
                continue;
            }
            decided
                .entry(r.int(0)?)
                .or_insert_with(|| vec![Vec::new(); pack.axes.len()])[a] =
                value.split(',').map(|v| v.trim().to_string()).collect();
        }

        let empty: Vec<Vec<String>> = vec![Vec::new(); pack.axes.len()];
        let mut axes: Vec<Vec<Param>> = Vec::new();
        let mut evidence: Vec<Vec<Param>> = Vec::new();
        for r in &rows {
            let (ids, stack) = to_stack(r, false)?;
            let modality =
                stack.text(nils_pack::stack::field_index("modality").expect("modality is a field"));
            if modality != pack.modality {
                continue;
            }
            let seed = decided.get(&ids.stack).unwrap_or(&empty);
            let verdict = Evaluated::new(pack, &stack).dispose(seed);
            for a in &verdict.axes {
                let value = a.stored();
                axes.push(vec![
                    Param::Int(ids.stack),
                    Param::from(a.axis.as_str()),
                    if value.is_empty() {
                        Param::Null
                    } else {
                        Param::from(value.as_str())
                    },
                    Param::Double(a.confidence),
                    Param::from(a.tier.as_str()),
                ]);
            }
            for e in &verdict.evidence {
                evidence.push(vec![
                    Param::Int(ids.stack),
                    Param::from(e.axis.as_str()),
                    Param::from(e.value.as_str()),
                    Param::from(e.tier.as_str()),
                    Param::Double(e.confidence),
                    Param::from(e.rule_set.as_str()),
                    Param::from(e.rule.as_str()),
                    Param::from(e.source.as_str()),
                    if e.matched.is_empty() {
                        Param::Null
                    } else {
                        Param::from(e.matched.as_str())
                    },
                ]);
            }
            disposed += 1;
        }

        if !axes.is_empty() {
            store.begin()?;
            let write = (|| -> Result<(), nils_registry::store::Error> {
                // Only this phase's rows: the class phase's answers are what
                // this one was worked out from.
                for t in [axis_t, ev_t] {
                    store.execute(
                        &format!(
                            "DELETE FROM {} WHERE stack_id > {} AND stack_id <= {} AND {scope}",
                            store.qualified(t.name),
                            store.dialect().param(1, Type::Int),
                            store.dialect().param(2, Type::Int),
                        ),
                        &[Param::Int(after), Param::Int(last)],
                    )?;
                }
                store.insert(
                    &Insert::new(axis_t, &["stack_id", "axis", "value", "confidence", "tier"]),
                    &axes,
                )?;
                store.insert(
                    &Insert::new(
                        ev_t,
                        &[
                            "stack_id",
                            "axis",
                            "value",
                            "tier",
                            "confidence",
                            "rule_set",
                            "rule",
                            "source",
                            "matched",
                        ],
                    ),
                    &evidence,
                )?;
                Ok(())
            })();
            match write {
                Ok(()) => store.commit()?,
                Err(e) => {
                    store.rollback().ok();
                    return Err(Error::Store(e));
                }
            }
        }

        crate::job::beat(store, job_id)?;
        after = last;
        if rows.len() < window {
            break;
        }
    }
    Ok(disposed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_line_up_with_the_ones_a_pack_names() {
        // A pass turns a field index from the pack's list into a column with
        // this one. They are two lists because one belongs to a crate that
        // knows nothing about a registry, and they only work because they are
        // the same length in the same order. Adding to one and not the other
        // makes a rule read a neighbouring column and say nothing about it,
        // which is what happened when Wave 3 §6's fields landed.
        assert_eq!(FIELDS.len(), nils_pack::stack::FIELDS.len());
        for (i, (name, _)) in FIELDS.iter().enumerate() {
            assert_eq!(*name, nils_pack::stack::FIELDS[i], "field {i}");
        }
    }

    #[test]
    fn every_field_is_a_fingerprint_column() {
        let t = table("stack_fingerprint");
        for (name, column) in FIELDS {
            assert!(t.column(column).is_some(), "{name} reads {column}");
        }
    }
}
