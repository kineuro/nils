// SPDX-License-Identifier: AGPL-3.0-only

//! Running a pack's picks over a registry
//! (`docs/specs/wave3-anonymize-and-bids.md`, §10).
//!
//! The model and the scoring are `nils_pack`'s, so a pack can be checked
//! against a fixture by somebody who has never seen this schema. What is here
//! is the registry: which rows to read, how a session is derived, what the
//! population is, and what is written down.
//!
//! Three things this does that v0 does not.
//!
//! It says which **population** the cohort-relative components were scored
//! against. Three of the eight read one, so the same stack scored against two
//! cohorts gets two answers; v0 records neither the population nor the fact
//! that it read one, so its picks cannot be reproduced from what is stored.
//!
//! It **reports a tie** instead of settling it by row order. v0 sorts its
//! bundles and takes the first, so a session whose two best differ by nothing
//! gets whichever the database returned, and the same cohort re-run can return
//! the other.
//!
//! And it names the **scheme** the session came from, because a session is
//! derived (§5) and the same studies are one occasion or two depending on it.

use std::collections::{BTreeMap, HashMap};

use nils_pack::pack::Pack;
use nils_pack::pick::{self, Candidate, Model, Reference};
use nils_registry::review;
use nils_registry::schema::{Type, table};
use nils_registry::session::{self, Scheme};
use nils_registry::store::{Error as StoreError, Insert, Param, Store};
use nils_registry::{Registry, day::Day, time::now_iso};

use crate::Error;

/// What a run of the picks says when it is done.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Picked {
    /// Occasions looked at, per role.
    pub sessions: i64,
    /// Picks written.
    pub written: i64,
    /// Of those, the ones a person should look at, by reason.
    pub borders: BTreeMap<String, i64>,
    /// Occasions where the role had no candidate at all.
    pub empty: i64,
    /// Record 42 S3: occasions where a person's pick stands. The run still
    /// writes its own pick there, as evidence that does not apply.
    pub standing: i64,
    /// Occasions with an open `pick.border` review item after the run.
    pub raised: i64,
    /// The population each role was scored against.
    pub reference: String,
    /// Record 55 H2: the subjects whose occasions this run decided, when it
    /// decided some and not all: `cohort:NAME` or `dataset:NAME`. They are
    /// scored against the whole registry as any run is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub only: Option<String>,
    /// How many subjects `only` named.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subjects: Option<usize>,
    /// Record 55 H2 (round 4): the datasets whose subjects `only` named,
    /// where it was made from them (a pick run after a sort).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub datasets: Vec<String>,
    /// Record 55 H2 (round 4): occasions whose two best candidates scored
    /// the same, which the run reports and does not settle by row order.
    pub tied: i64,
    /// Record 55 H2 (round 4): the same counts for each role, so that a
    /// page can say "T1: 112 picked, 104 clear, 8 borders".
    pub roles: BTreeMap<String, RoleReport>,
    /// The job the run was, which keeps this report as its result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<i64>,
    pub seconds: f64,
}

/// What a run did for one role (record 55 H2, round 4).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct RoleReport {
    /// Occasions looked at.
    pub sessions: i64,
    /// Picks written.
    pub picked: i64,
    /// Picks written with no border, no tie and no person's pick standing
    /// over them: nothing for a person to look at.
    pub clear: i64,
    /// Occasions with no candidate at all.
    pub empty: i64,
    /// Occasions whose two best candidates scored the same.
    pub tied: i64,
    /// Occasions where a person's pick stands.
    pub standing: i64,
    /// Occasions with an open `pick.border` review item after the run.
    pub raised: i64,
    /// The borders, by reason.
    pub borders: BTreeMap<String, i64>,
}

/// Whether a run's two best candidates scored the same: a tie, which the
/// run reports and never settles by row order.
pub fn is_tied(picked: &pick::Picked) -> bool {
    picked.winner.is_some()
        && picked.decided.is_none()
        && picked.considered.len() >= 2
        && picked.considered[0].1 == picked.considered[1].1
}

/// The stacks of a role: those holding it that the pick's candidacy for it
/// admits (pack contract 9). They are its candidates and the population it
/// is scored against.
pub(crate) fn of_role<'a>(model: &Model, role: &str, rows: &'a [Row]) -> Vec<&'a Row> {
    rows.iter()
        .filter(|r| r.roles.iter().any(|x| x == role) && model.admits(role, &r.values))
        .collect()
}

/// Record 55 H2: the subjects a run decides, named for the report: a
/// cohort's open members or a dataset's subjects. The population a role is
/// scored against stays the registry's, so a pick does not change with the
/// part of the registry it was run for.
#[derive(Debug, Clone)]
pub struct Only {
    pub label: String,
    pub subjects: std::collections::BTreeSet<i64>,
    /// The datasets the subjects were found through, named in the report
    /// (record 55 H2, round 4: a pick run after a sort); empty otherwise.
    pub datasets: Vec<String>,
}

/// One stack, as the picks need it.
pub(crate) struct Row {
    pub(crate) stack: i64,
    pub(crate) subject: i64,
    pub(crate) study: i64,
    pub(crate) values: BTreeMap<String, String>,
    pub(crate) roles: Vec<String>,
    pub(crate) scan: Scan,
}

impl Row {
    /// The stack with what [`scans`] read of it. The 2026-10-10 borders
    /// study, R5: the pick reads a stack's `n_instances` as the images it
    /// holds, so an enhanced multi-frame file, one instance of many frames,
    /// is the volume of its frames and not a volume of one slice. Where the
    /// stack holds no more images than instances, nothing changes.
    pub(crate) fn with_scan(mut self, scan: Scan) -> Row {
        if let Some(images) = scan.images
            && let Some(n) = self
                .values
                .get("n_instances")
                .and_then(|v| v.trim().parse::<f64>().ok())
            && images as f64 > n
        {
            self.values
                .insert("n_instances".to_string(), images.to_string());
        }
        self.scan = scan;
        self
    }
}

/// What the picks read of a stack beside the names the pack's model reads.
#[derive(Debug, Clone, Default)]
pub(crate) struct Scan {
    /// How many images it holds: its instances, each frame of an enhanced
    /// multi-frame file counted as one (R5). None where the fingerprint
    /// says nothing.
    pub(crate) images: Option<i64>,
    /// The series it came out of, and the number the scanner gave it.
    pub(crate) series: i64,
    pub(crate) series_number: Option<i64>,
    /// When its images were first acquired, the date and the time as the
    /// fingerprint writes them (record 38 S2): one moment is one
    /// acquisition (R6).
    pub(crate) acquired: Option<String>,
    /// Whether its ImageType says DERIVED rather than ORIGINAL.
    pub(crate) derived: bool,
}

/// The SOP classes whose image is one frame by definition, the MR and CT
/// images a scanner writes by default: a stack of them holds as many images
/// as instances, and its files are not read for frames.
const ONE_FRAME: &[&str] = &["1.2.840.10008.5.1.4.1.1.4", "1.2.840.10008.5.1.4.1.1.2"];

/// How many stacks one query of [`scans`] names.
const SCANS_CHUNK: usize = 500;

/// A [`Scan`] of each of `stacks` that has a fingerprint. The images of a
/// stack whose SOP class may hold several frames are counted from its
/// files: every frame of an instance that names the stack, and where one
/// file's frames were split over several stacks (record 37 S8), the frames
/// `instance_frame` gives each.
pub(crate) fn scans(store: &mut Store, stacks: &[i64]) -> Result<HashMap<i64, Scan>, StoreError> {
    let mut out: HashMap<i64, Scan> = HashMap::new();
    let mut framed: Vec<i64> = Vec::new();
    let fp = table("stack_fingerprint");
    let text = |column: &str| {
        store
            .dialect()
            .text_of(fp.column(column).expect("a fingerprint column"))
    };
    let (date, time) = (
        text("earliest_acquisition_date"),
        text("earliest_acquisition_time"),
    );
    for chunk in stacks.chunks(SCANS_CHUNK) {
        let sql = format!(
            "SELECT stack_id, n_instances, sop_class_uid, series_id, series_number, {date}, {time}, \
                    image_type FROM {} WHERE stack_id IN ({})",
            store.qualified("stack_fingerprint"),
            id_list(chunk)
        );
        for r in store.query(&sql, &[])? {
            let stack = r.int(0)?;
            if r.opt_text(2)?
                .is_none_or(|class| !ONE_FRAME.contains(&class.trim()))
            {
                framed.push(stack);
            }
            // a time without its day is a moment still; a day without its
            // time is none
            let acquired = match (r.opt_text(5)?, r.opt_text(6)?) {
                (_, None) => None,
                (Some(day), Some(time)) => Some(format!("{day} {time}")),
                (None, Some(time)) => Some(time.to_string()),
            };
            out.insert(
                stack,
                Scan {
                    images: r.opt_int(1)?,
                    series: r.int(3)?,
                    series_number: r.opt_int(4)?,
                    acquired,
                    derived: r.opt_text(7)?.is_some_and(|t| {
                        t.split('\\')
                            .next()
                            .is_some_and(|v| v.trim().eq_ignore_ascii_case("DERIVED"))
                    }),
                },
            );
        }
    }
    for chunk in framed.chunks(SCANS_CHUNK) {
        let list = id_list(chunk);
        let sql = format!(
            "SELECT stack_id, CAST(SUM(n) AS BIGINT) FROM (\
               SELECT i.stack_id AS stack_id, COALESCE(i.number_of_frames, 1) AS n FROM {i} i \
                WHERE i.stack_id IN ({list}) \
                  AND NOT EXISTS (SELECT 1 FROM {fr} fr WHERE fr.instance_id = i.id) \
               UNION ALL \
               SELECT fr.stack_id AS stack_id, fr.n_frames AS n FROM {fr} fr \
                WHERE fr.stack_id IN ({list})\
             ) t GROUP BY stack_id",
            i = store.qualified("instance"),
            fr = store.qualified("instance_frame"),
        );
        for r in store.query(&sql, &[])? {
            let (stack, frames) = (r.int(0)?, r.opt_int(1)?);
            if let (Some(scan), Some(frames)) = (out.get_mut(&stack), frames) {
                scan.images = Some(scan.images.map_or(frames, |n| n.max(frames)));
            }
        }
    }
    Ok(out)
}

fn id_list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Run every pick the pack declares.
/// A pick is a job (Wave 4a §9.1).
pub fn run(
    registry: &mut Registry,
    pack: &Pack,
    scheme: &Scheme,
    subject: Option<&str>,
    actor: &str,
) -> Result<Picked, Error> {
    run_for(registry, pack, scheme, subject, None, actor)
}

/// [`run`] deciding the occasions of `only`'s subjects alone (record 55
/// H2: a pick run for a dataset or a cohort, as the bring-in runs it).
pub fn run_for(
    registry: &mut Registry,
    pack: &Pack,
    scheme: &Scheme,
    subject: Option<&str>,
    only: Option<&Only>,
    actor: &str,
) -> Result<Picked, Error> {
    let settings = crate::job::Settings {
        name: only
            .map(|o| o.label.clone())
            .or_else(|| subject.map(str::to_string))
            .unwrap_or_else(|| "all".to_string()),
        ..crate::job::Settings::default()
    };
    let job_id = crate::job::claim_for(registry, &settings, "pick")?;
    let mut result = run_pick(registry, pack, scheme, subject, only, actor, Some(job_id));
    let (state, error) = match &mut result {
        Ok(report) => {
            // record 55 H2 (round 4): the job keeps its report, which the
            // picks summary door serves beside the picks themselves
            report.job = Some(job_id);
            if let Ok(doc) = serde_json::to_value(&*report) {
                let _ = nils_registry::job::set_result(registry.store(), job_id, &doc);
            }
            ("done", None)
        }
        Err(e) => ("failed", Some(e.to_string())),
    };
    let _ = crate::job::finish(registry.store(), job_id, state, error.as_deref());
    result
}

fn run_pick(
    registry: &mut Registry,
    pack: &Pack,
    scheme: &Scheme,
    subject: Option<&str>,
    only: Option<&Only>,
    actor: &str,
    job_id: Option<i64>,
) -> Result<Picked, Error> {
    let started = std::time::Instant::now();
    let mut report = Picked::default();
    if let Some(o) = only {
        report.only = Some(o.label.clone());
        report.subjects = Some(o.subjects.len());
        report.datasets = o.datasets.clone();
    }
    if pack.picks.is_empty() {
        return Ok(report);
    }
    report.reference = match subject {
        Some(code) => format!("subject:{code}"),
        None => "registry".to_string(),
    };

    // Wave 4b §7: the occasions come from the session cache, built over each
    // subject's whole timeline under the scheme's own anchor, and not from
    // this role's candidates, so a pick keys on the day every reader sees.
    let anchors =
        nils_session::Anchors::resolve(registry, scheme, BTreeMap::new()).map_err(session_err)?;
    nils_session::ensure(registry, scheme, &anchors, subject, false).map_err(session_err)?;
    let store = registry.store();
    let labels = nils_session::labels_by_study(store, scheme).map_err(session_err)?;
    for model in &pack.picks {
        let rows = read_rows(store, model, subject)?;
        run_one(
            store,
            model,
            pack,
            scheme,
            &labels,
            &rows,
            only.map(|o| &o.subjects),
            actor,
            job_id,
            &mut report,
        )?;
    }
    report.seconds = started.elapsed().as_secs_f64();
    Ok(report)
}

/// The day each study happened on, which is what a session is grouped by.
fn session_err(e: nils_session::Error) -> Error {
    match e {
        nils_session::Error::Store(s) => Error::Store(s),
        nils_session::Error::Message(m) => Error::Store(StoreError::Message(m)),
    }
}

/// Every stack that holds a role, with the values this model reads.
fn read_rows(store: &mut Store, model: &Model, subject: Option<&str>) -> Result<Vec<Row>, Error> {
    let reads = model.reads();
    // A name is either a fingerprint column or an axis. The fingerprint half
    // is read in one pass over the table; the axes in one pass over theirs.
    let mut columns = vec![
        "f.stack_id".to_string(),
        "f.subject_id".to_string(),
        "f.study_id".to_string(),
    ];
    let mut fields: Vec<String> = Vec::new();
    for name in &reads {
        let Some(field) = crate::classify::FIELDS.iter().find(|(n, _)| n == name) else {
            continue;
        };
        columns.push(crate::classify::field_sql(store, Some("f"), *field));
        fields.push(name.clone());
    }
    let filter = match subject {
        Some(code) => format!(
            " JOIN {} su ON su.id = f.subject_id AND su.code = '{}'",
            store.qualified("subject"),
            code.replace('\'', "''")
        ),
        None => String::new(),
    };
    let sql = format!(
        "SELECT {} FROM {} f{filter} ORDER BY f.stack_id",
        columns.join(", "),
        store.qualified("stack_fingerprint"),
    );
    let mut rows: Vec<Row> = Vec::new();
    for r in store.query(&sql, &[])? {
        let mut values = BTreeMap::new();
        for (i, name) in fields.iter().enumerate() {
            if let Some(v) = crate::classify::cell_text(r.get(i + 3))
                && !v.is_empty()
            {
                values.insert(name.clone(), v);
            }
        }
        rows.push(Row {
            stack: r.int(0)?,
            subject: r.int(1)?,
            study: r.int(2)?,
            values,
            roles: Vec::new(),
            scan: Scan::default(),
        });
    }

    // The axes, including `role`, which is what says a stack is a candidate.
    let sql = format!(
        "SELECT stack_id, axis, value FROM {}",
        store.qualified("classification_axis")
    );
    let mut by_stack: HashMap<i64, usize> = HashMap::new();
    for (i, r) in rows.iter().enumerate() {
        by_stack.insert(r.stack, i);
    }
    for r in store.query(&sql, &[])? {
        let Some(i) = by_stack.get(&r.int(0)?).copied() else {
            continue;
        };
        let axis = r.text(1)?;
        let Some(value) = r.opt_text(2)? else {
            continue;
        };
        // One row per value (Wave 4a §6.1): a role arrives as rows, and an
        // axis a pick reads is joined back into the text the model matches
        // a token against.
        if axis == nils_pack::matters::PICK_CANDIDATES {
            rows[i].roles.push(value.trim().to_string());
        }
        if reads.iter().any(|n| n == axis) {
            rows[i]
                .values
                .entry(axis.to_string())
                .and_modify(|v| {
                    v.push(',');
                    v.push_str(value);
                })
                .or_insert_with(|| value.to_string());
        }
    }
    rows.retain(|r| !r.roles.is_empty());
    let ids: Vec<i64> = rows.iter().map(|r| r.stack).collect();
    let mut scans = scans(store, &ids)?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let scan = scans.remove(&r.stack).unwrap_or_default();
            r.with_scan(scan)
        })
        .collect())
}

#[allow(clippy::too_many_arguments)]
fn run_one(
    store: &mut Store,
    model: &Model,
    pack: &Pack,
    scheme: &Scheme,
    labels: &HashMap<i64, nils_session::Labelled>,
    rows: &[Row],
    only: Option<&std::collections::BTreeSet<i64>>,
    actor: &str,
    job_id: Option<i64>,
    report: &mut Picked,
) -> Result<(), Error> {
    let now = now_iso();
    let scheme_json = serde_json::to_string(scheme).unwrap_or_default();
    let scheme_name = short_scheme(scheme);
    let scheme_digest = scheme.digest();
    // record 51 R2: where the run looked, named on every border it raises
    let origin = Origin {
        scheme: serde_json::json!({
            "name": scheme_name, "digest": scheme_digest,
            "definition": serde_json::to_value(scheme).unwrap_or(serde_json::Value::Null),
        }),
        pack: pack.name.clone(),
        pack_version: pack.version.to_string(),
    };

    for role in &model.roles {
        let mine: Vec<&Row> = of_role(model, role, rows);
        // The population is every stack of this role, a fragment no
        // candidate is made of included, and it is named on every row it
        // decided.
        let reference = build_reference(model, &report.reference, &mine);

        // Which studies are one occasion, per subject.
        let mut by_subject: BTreeMap<i64, Vec<&Row>> = BTreeMap::new();
        for r in &mine {
            by_subject.entry(r.subject).or_default().push(r);
        }
        for (subject, subject_rows) in &by_subject {
            // the population is everyone's; the occasions decided are only's
            if only.is_some_and(|o| !o.contains(subject)) {
                continue;
            }
            let mut occasions: BTreeMap<i64, (Day, Vec<&Row>)> = BTreeMap::new();
            for r in subject_rows {
                let Some(l) = labels.get(&r.study) else {
                    continue;
                };
                occasions
                    .entry(l.session_id)
                    .or_insert_with(|| (l.first, Vec::new()))
                    .1
                    .push(r);
            }
            for (first, here) in occasions.values() {
                report.sessions += 1;
                report.roles.entry(role.clone()).or_default().sessions += 1;
                let candidates = group(model, here);
                let picked = pick::pick(model, role, &candidates, &reference);
                for b in &picked.borders {
                    *report.borders.entry(b.name().to_string()).or_insert(0) += 1;
                    *report
                        .roles
                        .entry(role.clone())
                        .or_default()
                        .borders
                        .entry(b.name().to_string())
                        .or_insert(0) += 1;
                }
                let tied = is_tied(&picked);
                if tied {
                    report.tied += 1;
                    report.roles.entry(role.clone()).or_default().tied += 1;
                }
                // Record 42 S3: a person's pick on this occasion stands. The
                // run neither replaces nor withdraws it, and asks nothing
                // about an occasion a person already answered.
                let standing = standing_person(store, &model.name, role, *subject, *first)?;
                let pick_id = if picked.winner.is_none() {
                    report.empty += 1;
                    report.roles.entry(role.clone()).or_default().empty += 1;
                    None
                } else {
                    let id = write(
                        store,
                        model,
                        pack,
                        &picked,
                        *subject,
                        *first,
                        &scheme_name,
                        &scheme_json,
                        &scheme_digest,
                        &reference.name,
                        actor,
                        &now,
                        standing,
                        job_id,
                    )?;
                    report.written += 1;
                    let r = report.roles.entry(role.clone()).or_default();
                    r.picked += 1;
                    if picked.borders.is_empty() && !tied && standing.is_none() {
                        r.clear += 1;
                    }
                    Some(id)
                };
                if standing.is_some() {
                    report.standing += 1;
                    report.roles.entry(role.clone()).or_default().standing += 1;
                    continue;
                }
                if border_item(
                    store, model, &picked, *subject, *first, pick_id, actor, job_id, &origin,
                )? {
                    report.raised += 1;
                    report.roles.entry(role.clone()).or_default().raised += 1;
                }
            }
        }
    }
    Ok(())
}

/// A short name for the scheme, so a row says which one it was made under
/// without carrying the whole of it.
fn short_scheme(scheme: &Scheme) -> String {
    let naming = match &scheme.naming {
        session::Naming::Date => "date".to_string(),
        session::Naming::Ordinal => "ordinal".to_string(),
        session::Naming::Months { cadence, tolerance } => format!(
            "months[{}]+-{tolerance}",
            cadence
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
    };
    format!("window={}d,{naming}", scheme.window_days)
}

/// Two stacks of one acquisition are one candidate, and the outputs of one
/// acquisition are merged back into one after that.
pub(crate) fn group(model: &Model, rows: &[&Row]) -> Vec<Candidate> {
    let key_of = |r: &Row, ignoring: &[String], over: Option<&str>| -> String {
        model
            .same_acquisition
            .iter()
            .filter(|n| Some(n.as_str()) != over && !ignoring.contains(n))
            .map(|n| r.values.get(n).cloned().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\u{1}")
    };

    // Stage one: the full key. A fragment of a few images is no candidate
    // and no part of one (the 2026-10-10 borders study, R6): a spin echo
    // split into stacks of 2, 19 and 1 images is a take of 19.
    let mut groups: BTreeMap<String, Vec<&Row>> = BTreeMap::new();
    for r in rows {
        if r.scan
            .images
            .is_some_and(|n| n as f64 <= pick::FRAGMENT_IMAGES)
        {
            continue;
        }
        groups.entry(key_of(r, &[], None)).or_default().push(r);
    }

    // Stage two: the outputs of one acquisition, merged, per family, and a
    // stack belongs to the first family whose token it holds. Only where
    // more than one output exists; a lone in-phase image is one acquisition
    // either way.
    let mut plain: Vec<Vec<&Row>> = Vec::new();
    let mut families: BTreeMap<(usize, String), Vec<Vec<&Row>>> = BTreeMap::new();
    for (_, members) in groups {
        let family = model.families.iter().position(|f| {
            members[0]
                .values
                .get(&f.when.0)
                .is_some_and(|v| holds(v, &f.when.1))
        });
        match family {
            None => plain.push(members),
            Some(i) => {
                let f = &model.families[i];
                families
                    .entry((i, key_of(members[0], &f.ignoring, Some(&f.over))))
                    .or_default()
                    .push(members);
            }
        }
    }

    let mut out: Vec<Candidate> = Vec::new();
    for members in plain {
        out.push(candidate(model, &members, None));
    }
    for ((i, _), parts) in families {
        let f = &model.families[i];
        let all: Vec<&Row> = parts.iter().flatten().copied().collect();
        if all.len() == 1 {
            out.push(candidate(model, &all, None));
            continue;
        }
        // Within a family, only the variants worth keeping, best first.
        let mut kept: Vec<&Row> = Vec::new();
        for want in &f.canonical {
            kept = all
                .iter()
                .filter(|r| r.values.get(&f.over).is_some_and(|v| holds(v, want)))
                .copied()
                .collect();
            if !kept.is_empty() {
                break;
            }
        }
        if !kept.is_empty() {
            out.push(candidate(model, &kept, Some(&f.name)));
        } else if f.apart_without_canonical {
            // v0's MP2RAGE with no labelled output: the acquisitions stand
            // as they were before the merge.
            for members in parts {
                out.push(candidate(model, &members, None));
            }
        }
        // And otherwise, v0's Dixon: a family with none of them is not a
        // candidate at all.
    }
    out
}

/// R6 of the 2026-10-10 borders study, after record 38's ruling on what a
/// rescan is: series of one candidate acquired at one moment are one
/// acquisition stored twice, a second reconstruction or a re-send, so the
/// candidate keeps one of them, the scanner's original image over a derived
/// one and then the series it wrote first. The stacks of one series are its
/// parts and stay together, and a stack whose images carry no time is a
/// moment of its own, because an absence is not a measurement.
fn one_series_a_moment<'a>(rows: &[&'a Row]) -> Vec<&'a Row> {
    let mut kept: BTreeMap<&str, (bool, i64, i64)> = BTreeMap::new();
    for r in rows {
        let Some(moment) = r.scan.acquired.as_deref() else {
            continue;
        };
        let rank = (
            r.scan.derived,
            r.scan.series_number.unwrap_or(i64::MAX),
            r.scan.series,
        );
        kept.entry(moment)
            .and_modify(|best| *best = (*best).min(rank))
            .or_insert(rank);
    }
    rows.iter()
        .copied()
        .filter(|r| {
            r.scan
                .acquired
                .as_deref()
                .is_none_or(|m| kept[m].2 == r.scan.series)
        })
        .collect()
}

/// One candidate of the rows of one acquisition: what they agree on, each
/// number at its largest, because a bundle's slice count is the fullest
/// volume in it, and each stack's own values beside.
fn candidate(model: &Model, kept: &[&Row], family: Option<&str>) -> Candidate {
    let kept = one_series_a_moment(kept);
    let reads = model.reads();
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    for name in &reads {
        let mut best: Option<String> = None;
        for r in &kept {
            let Some(v) = r.values.get(name) else {
                continue;
            };
            best = Some(match (best, v.parse::<f64>(), v) {
                (None, _, v) => v.clone(),
                (Some(b), Ok(n), v) => match b.parse::<f64>() {
                    Ok(m) if n > m => v.clone(),
                    Ok(_) => b,
                    Err(_) => b,
                },
                (Some(b), Err(_), _) => b,
            });
        }
        if let Some(v) = best {
            values.insert(name.clone(), v);
        }
    }
    let mut each: Vec<(i64, BTreeMap<String, String>, Option<String>)> = kept
        .iter()
        .map(|r| {
            (
                r.stack,
                r.values
                    .iter()
                    .filter(|(k, _)| reads.contains(k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                r.scan.acquired.clone(),
            )
        })
        .collect();
    each.sort_by_key(|(s, _, _)| *s);
    Candidate {
        stacks: each.iter().map(|(s, _, _)| *s).collect(),
        values,
        acquired: each.iter().map(|(_, _, m)| m.clone()).collect(),
        each: each.into_iter().map(|(_, v, _)| v).collect(),
        family: family.map(str::to_string),
    }
}

fn holds(csv: &str, token: &str) -> bool {
    csv.split(',').any(|t| t.trim().eq_ignore_ascii_case(token))
}

/// What the population says about itself.
pub(crate) fn build_reference(model: &Model, name: &str, rows: &[&Row]) -> Reference {
    let mut counts: BTreeMap<String, BTreeMap<String, i64>> = BTreeMap::new();
    for name in model.reads() {
        let mut per: BTreeMap<String, i64> = BTreeMap::new();
        for r in rows {
            if let Some(v) = r.values.get(&name)
                && !v.is_empty()
            {
                *per.entry(v.clone()).or_insert(0) += 1;
            }
        }
        if !per.is_empty() {
            counts.insert(name, per);
        }
    }
    let mut percentiles = BTreeMap::new();
    for (population, of, split_by) in model.populations() {
        let mut buckets: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        for r in rows {
            let Some(v) = r.values.get(&of).and_then(|v| v.parse::<f64>().ok()) else {
                continue;
            };
            let key = match &split_by {
                Some(s) => format!(
                    "{population}:{}",
                    r.values.get(s).cloned().unwrap_or_default()
                ),
                None => population.clone(),
            };
            buckets.entry(key).or_default().push(v);
        }
        for (key, values) in buckets {
            if let Some(p) = Reference::of(&values) {
                percentiles.insert(key, p);
            }
        }
    }
    Reference {
        name: name.to_string(),
        counts,
        percentiles,
        total: rows.len() as i64,
    }
}

#[allow(clippy::too_many_arguments)]
fn write(
    store: &mut Store,
    model: &Model,
    pack: &Pack,
    picked: &pick::Picked,
    subject: i64,
    day: Day,
    scheme_name: &str,
    scheme_json: &str,
    scheme_digest: &str,
    reference: &str,
    actor: &str,
    now: &str,
    standing: Option<i64>,
    job_id: Option<i64>,
) -> Result<i64, Error> {
    let winner = picked.winner.as_ref().expect("a pick with a winner");
    let scored = picked.scored.as_ref().expect("a winner is scored");
    let parts = serde_json::json!({
        "scheme": serde_json::from_str::<serde_json::Value>(scheme_json)
            .unwrap_or(serde_json::Value::Null),
        "penalty": scored.penalty,
        // Record 51: what each border found, by its name.
        "notes": notes_json(picked),
        "parts": scored
            .parts
            .iter()
            .map(|p| serde_json::json!({
                "name": p.name, "score": p.score, "weight": p.weight, "saw": p.saw,
            }))
            .collect::<Vec<_>>(),
    });
    let considered: Vec<serde_json::Value> = picked
        .considered
        .iter()
        .map(|(stacks, score)| serde_json::json!({"stacks": stacks, "score": score}))
        .collect();
    let borders: Vec<&str> = picked.borders.iter().map(|b| b.name()).collect();

    store.begin()?;
    let result = (|| -> Result<i64, StoreError> {
        // A run replaces what it decided before for this role and occasion.
        // What a person decided is a withdrawal, not a row this can reach.
        let d = store.dialect();
        let sql = format!(
            "DELETE FROM {} WHERE pick_id IN (SELECT id FROM {} \
             WHERE model = {} AND role = {} AND subject_id = {} AND session_day = {} \
               AND author_kind = 'agent')",
            store.qualified("pick_stack"),
            store.qualified("pick"),
            d.param(1, Type::Text),
            d.param(2, Type::Text),
            d.param(3, Type::Int),
            d.param(4, Type::Date),
        );
        let key = [
            Param::from(model.name.as_str()),
            Param::from(picked.role.as_str()),
            Param::Int(subject),
            Param::from(day.to_string()),
        ];
        store.execute(&sql, &key)?;
        let sql = format!(
            "DELETE FROM {} WHERE model = {} AND role = {} AND subject_id = {} \
               AND session_day = {} AND author_kind = 'agent'",
            store.qualified("pick"),
            d.param(1, Type::Text),
            d.param(2, Type::Text),
            d.param(3, Type::Int),
            d.param(4, Type::Date),
        );
        store.execute(&sql, &key)?;

        let written = store.insert(
            &Insert::new(
                table("pick"),
                &[
                    "model",
                    "role",
                    "subject_id",
                    "session_day",
                    "scheme",
                    "scheme_digest",
                    "score",
                    "margin",
                    "runner_up_score",
                    "borders",
                    "parts",
                    "considered",
                    "reference",
                    "pack",
                    "pack_version",
                    "actor",
                    "author_kind",
                    "decided_at",
                    "job_id",
                    "withdrawn_at",
                    "overruled_by",
                ],
            )
            .returning(&["id"]),
            &[vec![
                Param::from(model.name.as_str()),
                Param::from(picked.role.as_str()),
                Param::Int(subject),
                Param::from(day.to_string()),
                Param::from(scheme_name),
                Param::from(scheme_digest),
                Param::Double(scored.score),
                Param::Double(picked.margin),
                Param::Double(picked.runner_up_score),
                if borders.is_empty() {
                    Param::Null
                } else {
                    Param::from(borders.join(","))
                },
                Param::from(parts.to_string()),
                Param::from(serde_json::Value::Array(considered).to_string()),
                Param::from(reference),
                Param::from(pack.name.as_str()),
                Param::from(pack.version.to_string()),
                Param::from(actor),
                // §10.1: an automatic pick is an agent's, and says so, so that
                // a person's call is distinguishable from it wherever it is
                // read.
                Param::from("agent"),
                Param::from(now),
                job_id.map_or(Param::Null, Param::Int),
                // Record 42 S3: under a person's pick the run's own is kept
                // as evidence and does not apply.
                if standing.is_some() {
                    Param::from(now)
                } else {
                    Param::Null
                },
                standing.map_or(Param::Null, Param::Int),
            ]],
        )?;
        let id = written.first().map(|r| r.int(0)).transpose()?.unwrap_or(0);
        store.insert(
            &Insert::new(table("pick_stack"), &["pick_id", "stack_id"]),
            &winner
                .stacks
                .iter()
                .map(|s| vec![Param::Int(id), Param::Int(*s)])
                .collect::<Vec<_>>(),
        )?;
        Ok(id)
    })();
    match result {
        Ok(id) => {
            store.commit()?;
            Ok(id)
        }
        Err(e) => {
            store.rollback().ok();
            Err(Error::Store(e))
        }
    }
}

/// The person's pick that stands on one role and occasion, if any.
fn standing_person(
    store: &mut Store,
    model: &str,
    role: &str,
    subject: i64,
    day: Day,
) -> Result<Option<i64>, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT id FROM {} WHERE model = {} AND role = {} AND subject_id = {} \
           AND session_day = {} AND author_kind = 'person' AND withdrawn_at IS NULL \
         ORDER BY id DESC LIMIT 1",
        store.qualified("pick"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
        d.param(3, Type::Int),
        d.param(4, Type::Date),
    );
    store
        .query_opt(
            &sql,
            &[
                Param::from(model),
                Param::from(role),
                Param::Int(subject),
                Param::from(day.to_string()),
            ],
        )?
        .map(|r| r.int(0))
        .transpose()
}

/// Where a run looked: the session scheme and the pack.
struct Origin {
    scheme: serde_json::Value,
    pack: String,
    pack_version: String,
}

/// Record 42 S3: raise, refresh or close the `pick.border` item of one
/// occasion. Answers whether one is open after it.
#[allow(clippy::too_many_arguments)]
fn border_item(
    store: &mut Store,
    model: &Model,
    picked: &pick::Picked,
    subject: i64,
    day: Day,
    pick_id: Option<i64>,
    actor: &str,
    job_id: Option<i64>,
    origin: &Origin,
) -> Result<bool, StoreError> {
    let day = day.to_string();
    let key = review::pick_border_key(&model.name, &picked.role, subject, &day);
    if picked.borders.is_empty() {
        review::close_pick_border(
            store,
            &key,
            review::RESOLVED,
            actor,
            &serde_json::json!({"why": "a later run found nothing to doubt", "pick_id": pick_id}),
        )?;
        return Ok(false);
    }
    let names: Vec<&str> = picked.borders.iter().map(|b| b.name()).collect();
    let considered = serde_json::Value::Array(
        picked
            .considered
            .iter()
            .map(|(stacks, score)| serde_json::json!({"stacks": stacks, "score": score}))
            .collect(),
    );
    review::raise_pick_border(
        store,
        &review::PickBorder {
            model: &model.name,
            role: &picked.role,
            subject_id: subject,
            day: &day,
            borders: &names,
            pick_id,
            score: picked.scored.as_ref().map(|s| s.score),
            margin: picked.winner.as_ref().map(|_| picked.margin),
            runner_up_score: picked.runner_up.as_ref().map(|_| picked.runner_up_score),
            considered: &considered,
            notes: &notes_json(picked),
            job_id,
            scheme: &origin.scheme,
            pack: &origin.pack,
            pack_version: &origin.pack_version,
        },
        &now_iso(),
    )?;
    Ok(true)
}

/// What each border of a pick found, by the border's name: the variant of a
/// retake, the stacks of a twin or of the plain candidate, the slice count
/// and its bounds.
fn notes_json(picked: &pick::Picked) -> serde_json::Value {
    serde_json::Value::Object(
        picked
            .notes
            .iter()
            .map(|(k, v)| ((*k).to_string(), serde_json::Value::from(v.as_str())))
            .collect(),
    )
}

// ------------------------------------------------------------- person picks

/// Why a person's pick or its withdrawal was not written.
#[derive(Debug)]
pub enum PersonError {
    /// The ask was wrong in a way the person can mend: said in words.
    Refused(String),
    Store(StoreError),
}

impl std::fmt::Display for PersonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PersonError::Refused(m) => write!(f, "{m}"),
            PersonError::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PersonError {}

impl From<StoreError> for PersonError {
    fn from(e: StoreError) -> Self {
        PersonError::Store(e)
    }
}

fn refused(m: impl Into<String>) -> PersonError {
    PersonError::Refused(m.into())
}

/// A person's pick (record 42 S3): the stacks that stand for a role on the
/// occasion they belong to, and why.
#[derive(Debug, Clone)]
pub struct PersonPick<'a> {
    pub role: &'a str,
    /// One acquisition: every stack the pick names, on one occasion of one
    /// subject.
    pub stacks: &'a [i64],
    /// The pack's pick that declares the role; the only one when omitted.
    pub model: Option<&'a str>,
    pub why: &'a str,
    /// The principal, as every provenance writer names it.
    pub actor: &'a str,
    /// The campaign the pick was closed from (record 42 S1's column).
    pub campaign: Option<i64>,
    /// The occasion the pick must be on, subject and day, when the caller
    /// asked about one (a pick campaign's item): stacks of another are
    /// refused before anything is written.
    pub occasion: Option<(i64, &'a str)>,
}

/// What a person's pick wrote.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PersonPicked {
    pub id: i64,
    pub model: String,
    pub role: String,
    pub subject_id: i64,
    pub session_day: String,
    pub stacks: Vec<i64>,
    /// The run's picks that stopped applying under it.
    pub overruled: Vec<i64>,
    /// An earlier person's pick on the same occasion it replaced.
    pub replaced: Vec<i64>,
    /// The `pick.border` items it answered.
    pub answered: u64,
}

/// Write a person's pick. It stands until a person withdraws it: a pick run
/// on the same occasion keeps its own pick as evidence that does not apply.
/// An earlier person's pick on the occasion is withdrawn by this one, and a
/// run's pick stops applying, pointing at this one, so that withdrawing it
/// lets the run's pick apply again.
pub fn set_person(
    registry: &mut Registry,
    pack: &Pack,
    scheme: &Scheme,
    p: &PersonPick<'_>,
) -> Result<PersonPicked, PersonError> {
    // Record 51 R2: a pick of no stack is written only by Keep on a border
    // where the run picked nothing, never by naming stacks.
    if p.stacks.is_empty() {
        return Err(refused("a pick names at least one stack"));
    }
    if p.why.trim().is_empty() {
        return Err(refused(
            "a person's pick says why; it is what a reader of the pick has in place of the run's scores",
        ));
    }
    // record 48, D1 of the move: no pick is written on a stack of a sample
    // sealed now
    let sealed = nils_registry::labels::sealed_for_writes(registry.store(), p.stacks)
        .map_err(|e| refused(e.to_string()))?;
    if !sealed.is_empty() {
        return Err(refused(format!(
            "stack(s) {} are of a sample sealed for certification; no pick is written on a sealed stack until a certificate unseals it (record 48)",
            sealed
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let declaring: Vec<&Model> = pack
        .picks
        .iter()
        .filter(|m| m.roles.iter().any(|r| r == p.role))
        .filter(|m| p.model.is_none_or(|n| n == m.name))
        .collect();
    let model = match declaring.as_slice() {
        [one] => *one,
        [] => {
            return Err(refused(format!(
                "{} declares no pick for the role {}{}",
                pack.id(),
                p.role,
                p.model.map(|m| format!(" in {m}")).unwrap_or_default()
            )));
        }
        _ => {
            return Err(refused(format!(
                "more than one pick of {} declares the role {}; name the pick",
                pack.id(),
                p.role
            )));
        }
    };

    // Where each stack sits: its subject and its study, and through the
    // scheme the occasion the study belongs to.
    let mut stacks: Vec<i64> = p.stacks.to_vec();
    stacks.sort_unstable();
    stacks.dedup();
    let store = registry.store();
    let list = stacks
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT f.stack_id, f.subject_id, f.study_id, su.code FROM {} f \
         JOIN {} su ON su.id = f.subject_id WHERE f.stack_id IN ({list})",
        store.qualified("stack_fingerprint"),
        store.qualified("subject"),
    );
    let rows = store.query(&sql, &[])?;
    if rows.len() != stacks.len() {
        let found: Vec<i64> = rows.iter().filter_map(|r| r.int(0).ok()).collect();
        let missing: Vec<String> = stacks
            .iter()
            .filter(|s| !found.contains(s))
            .map(i64::to_string)
            .collect();
        return Err(refused(format!(
            "no fingerprinted stack {}; a pick names stacks the fingerprint has read",
            missing.join(", ")
        )));
    }
    let subject = rows[0].int(1)?;
    let code = rows[0].text(3)?.to_string();
    if rows.iter().any(|r| r.int(1).ok() != Some(subject)) {
        return Err(refused("the stacks of one pick belong to one subject"));
    }
    let studies: Vec<i64> = rows.iter().filter_map(|r| r.int(2).ok()).collect();

    let anchors = nils_session::Anchors::resolve(registry, scheme, BTreeMap::new())
        .map_err(|e| refused(session_err(e).to_string()))?;
    nils_session::ensure(registry, scheme, &anchors, Some(&code), false)
        .map_err(|e| refused(session_err(e).to_string()))?;
    let store = registry.store();
    let labels = nils_session::labels_by_study(store, scheme)
        .map_err(|e| refused(session_err(e).to_string()))?;
    let mut occasions: Vec<(i64, Day)> = Vec::new();
    for study in &studies {
        let Some(l) = labels.get(study) else {
            return Err(refused(
                "a stack of the pick is on no session under the scheme; rebuild the sessions first",
            ));
        };
        if !occasions.iter().any(|(id, _)| *id == l.session_id) {
            occasions.push((l.session_id, l.first));
        }
    }
    let [(_, day)] = occasions.as_slice() else {
        return Err(refused(
            "the stacks of one pick are on one occasion; these are on more than one under the scheme",
        ));
    };
    let day = *day;
    let day_text = day.to_string();
    if let Some((want_subject, want_day)) = p.occasion
        && (want_subject != subject || want_day != day_text)
    {
        return Err(refused(format!(
            "the stacks are on subject {subject}'s occasion of {day_text}, and the pick was asked of subject {want_subject}'s of {want_day}"
        )));
    }
    let scheme_json = serde_json::to_value(scheme).unwrap_or(serde_json::Value::Null);
    write_person(
        registry,
        &PersonRow {
            model: &model.name,
            role: p.role,
            subject,
            day: &day_text,
            scheme_name: &short_scheme(scheme),
            scheme_digest: &scheme.digest(),
            scheme: &scheme_json,
            pack: &pack.name,
            pack_version: &pack.version.to_string(),
            stacks: &stacks,
            why: p.why,
            actor: p.actor,
            campaign: p.campaign,
            kept: None,
        },
    )
}

/// What a person's pick row is written from: the occasion's key, where the
/// pick came from, and the person's stacks and words. The one writer of a
/// person's pick, through which `nils pick set`, a pick campaign's close and
/// Keep on a `pick.border` item all go (record 51 R1).
struct PersonRow<'a> {
    model: &'a str,
    role: &'a str,
    subject: i64,
    day: &'a str,
    scheme_name: &'a str,
    scheme_digest: &'a str,
    /// The scheme's definition, kept in `parts` as a run's pick keeps it.
    scheme: &'a serde_json::Value,
    pack: &'a str,
    pack_version: &'a str,
    /// Empty only from Keep on a border whose run picked nothing (R2).
    stacks: &'a [i64],
    why: &'a str,
    actor: &'a str,
    campaign: Option<i64>,
    /// The `pick.border` item this pick keeps, which is marked accepted by
    /// the person as well as answered.
    kept: Option<i64>,
}

fn write_person(registry: &mut Registry, p: &PersonRow<'_>) -> Result<PersonPicked, PersonError> {
    use nils_registry::audit::{self, Action, Entry};

    let now = now_iso();
    let store = registry.store();
    store.begin()?;
    let written = (|| -> Result<PersonPicked, StoreError> {
        let d = store.dialect();
        let key = [
            Param::from(p.model),
            Param::from(p.role),
            Param::Int(p.subject),
            Param::from(p.day),
        ];
        let on_key = format!(
            "model = {} AND role = {} AND subject_id = {} AND session_day = {}",
            d.param(1, Type::Text),
            d.param(2, Type::Text),
            d.param(3, Type::Int),
            d.param(4, Type::Date),
        );
        let ids = |store: &mut Store, filter: &str| -> Result<Vec<i64>, StoreError> {
            let sql = format!(
                "SELECT id FROM {} WHERE {on_key} AND {filter} ORDER BY id",
                store.qualified("pick")
            );
            store.query(&sql, &key)?.iter().map(|r| r.int(0)).collect()
        };
        let replaced = ids(store, "author_kind = 'person' AND withdrawn_at IS NULL")?;
        let overruled = ids(store, "author_kind <> 'person' AND withdrawn_at IS NULL")?;

        let written = store.insert(
            &Insert::new(
                table("pick"),
                &[
                    "model",
                    "role",
                    "subject_id",
                    "session_day",
                    "scheme",
                    "scheme_digest",
                    "reference",
                    "pack",
                    "pack_version",
                    "actor",
                    "author_kind",
                    "decided_at",
                    "why",
                    "parts",
                    "campaign_id",
                ],
            )
            .returning(&["id"]),
            &[vec![
                Param::from(p.model),
                Param::from(p.role),
                Param::Int(p.subject),
                Param::from(p.day),
                Param::from(p.scheme_name),
                Param::from(p.scheme_digest),
                // A person's pick is scored against nothing: the population
                // is the run's word, and this one is a judgement.
                Param::from("person"),
                Param::from(p.pack),
                Param::from(p.pack_version),
                Param::from(p.actor),
                Param::from("person"),
                Param::from(now.as_str()),
                Param::from(p.why),
                Param::from(serde_json::json!({ "scheme": p.scheme }).to_string()),
                p.campaign.map_or(Param::Null, Param::Int),
            ]],
        )?;
        let id = written
            .first()
            .map(|r| r.int(0))
            .transpose()?
            .ok_or_else(|| StoreError::Message("the pick was not written back".into()))?;
        // Record 51 R2: a person's pick of no stack has no pick_stack rows;
        // it says that nothing stands for the role on this occasion.
        if !p.stacks.is_empty() {
            store.insert(
                &Insert::new(table("pick_stack"), &["pick_id", "stack_id"]),
                &p.stacks
                    .iter()
                    .map(|s| vec![Param::Int(id), Param::Int(*s)])
                    .collect::<Vec<_>>(),
            )?;
        }
        // An earlier person's pick is withdrawn by this one, and a run's
        // pick it had overruled now points here.
        for old in &replaced {
            store.update_by_id(
                table("pick"),
                &[
                    ("withdrawn_at", Param::from(now.as_str())),
                    ("withdrawn_by", Param::from(p.actor)),
                ],
                "id",
                *old,
            )?;
            let sql = format!(
                "UPDATE {} SET overruled_by = {} WHERE overruled_by = {}",
                store.qualified("pick"),
                d.param(1, Type::Int),
                d.param(2, Type::Int),
            );
            store.execute(&sql, &[Param::Int(id), Param::Int(*old)])?;
        }
        for run in &overruled {
            store.update_by_id(
                table("pick"),
                &[
                    ("withdrawn_at", Param::from(now.as_str())),
                    ("overruled_by", Param::Int(id)),
                ],
                "id",
                *run,
            )?;
        }
        let answered = review::close_pick_border(
            store,
            &review::pick_border_key(p.model, p.role, p.subject, p.day),
            "accepted",
            p.actor,
            &serde_json::json!({
                "pick_id": id, "stacks": p.stacks, "author_kind": "person", "actor": p.actor,
                "why": p.why, "kept": p.kept.is_some(),
            }),
        )?;
        if let Some(item) = p.kept {
            store.update_by_id(
                table("review_item"),
                &[
                    ("accepted_by", Param::from(p.actor)),
                    ("accepted_at", Param::from(now.as_str())),
                ],
                "id",
                item,
            )?;
        }
        Ok(PersonPicked {
            id,
            model: p.model.to_string(),
            role: p.role.to_string(),
            subject_id: p.subject,
            session_day: p.day.to_string(),
            stacks: p.stacks.to_vec(),
            overruled,
            replaced,
            answered,
        })
    })();
    let picked = match written {
        Ok(w) => w,
        Err(e) => {
            store.rollback().ok();
            return Err(e.into());
        }
    };
    store.commit()?;
    audit::record(
        registry,
        &Entry {
            principal: p.actor,
            action: Action::PickSet,
            scope: serde_json::json!({
                "pick": picked.id, "model": picked.model, "role": picked.role,
                "subject_id": picked.subject_id, "stacks": picked.stacks,
                "overruled": picked.overruled, "replaced": picked.replaced,
                "campaign": p.campaign, "kept": p.kept,
            }),
            policy: None,
            job_id: None,
            details: Some(serde_json::json!({ "why": p.why })),
        },
    )?;
    Ok(picked)
}

/// Record 51 R1 and R2: Keep on a `pick.border` item is a decision. It
/// writes a person's pick of the stacks the run picked on the item's
/// occasion, through the one person's-pick writer, with the person and a
/// why (theirs, or "kept the run's pick" naming the borders), and closes the
/// item as accepted. Like any person's pick it stands through later runs
/// until a person withdraws it. On a border where the run picked nothing
/// (`nothing_eligible`), the pick names no stack: nothing stands for the
/// role here, said by a person.
///
/// `seen` is the run's pick the person looked at, when the caller knows it
/// (`Some(None)`: a border where the run picked nothing): if a run has
/// picked again since, the keep is refused and nothing is written, so a
/// person never keeps a pick they did not see.
pub fn keep(
    registry: &mut Registry,
    item: i64,
    actor: &str,
    why: Option<&str>,
    seen: Option<Option<i64>>,
) -> Result<PersonPicked, PersonError> {
    let store = registry.store();
    let it = review::item(store, item)
        .map_err(|e| match e {
            review::Error::Store(s) => PersonError::Store(s),
            other => refused(other.to_string()),
        })?
        .ok_or_else(|| refused(format!("no review item {item}")))?;
    if it.kind != review::PICK_BORDER_KIND {
        return Err(refused(format!(
            "review item {item} is a {}, not a {}",
            it.kind,
            review::PICK_BORDER_KIND
        )));
    }
    if it.status != "open" {
        return Err(refused(format!(
            "review item {item} is already {}",
            it.status
        )));
    }
    let text = |k: &str| it.reference[k].as_str().map(str::to_string);
    let (Some(model), Some(role), Some(day), Some(subject)) = (
        text("model"),
        text("role"),
        text("session_day"),
        it.reference["subject_id"].as_i64(),
    ) else {
        return Err(refused(format!(
            "review item {item} does not name its occasion (model, role, subject and day)"
        )));
    };
    let borders: Vec<String> = it.evidence["borders"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| b.as_str().map(str::to_string))
        .collect();
    let named = it.evidence["pick_id"].as_i64();

    // The run's pick that applies on the occasion now.
    let d = store.dialect();
    let t = table("pick");
    let sql = format!(
        "SELECT id, scheme, scheme_digest, pack, pack_version, {} FROM {} \
         WHERE model = {} AND role = {} AND subject_id = {} AND session_day = {} \
           AND author_kind = 'agent' AND withdrawn_at IS NULL ORDER BY id DESC LIMIT 1",
        d.text_of(t.column("parts").expect("parts")),
        store.qualified("pick"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
        d.param(3, Type::Int),
        d.param(4, Type::Date),
    );
    let current = store.query_opt(
        &sql,
        &[
            Param::from(model.as_str()),
            Param::from(role.as_str()),
            Param::Int(subject),
            Param::from(day.as_str()),
        ],
    )?;
    let current_id = current.as_ref().map(|r| r.int(0)).transpose()?;
    // record 51 R1: a run since the person looked is a pick they did not see
    let changed = (named.is_some() && current_id != named) || seen.is_some_and(|s| s != named);
    if changed {
        return Err(refused(format!(
            "the run changed its pick on review item {item}'s occasion since it was read; look again"
        )));
    }
    let origin = &it.evidence["scheme"];
    let from_border = match (
        origin["name"].as_str(),
        origin["digest"].as_str(),
        it.evidence["pack"].as_str(),
        it.evidence["pack_version"].as_str(),
    ) {
        (Some(name), Some(digest), Some(pack), Some(version)) => Some((
            name.to_string(),
            digest.to_string(),
            origin["definition"].clone(),
            pack.to_string(),
            version.to_string(),
            Vec::new(),
        )),
        _ => None,
    };
    let (scheme_name, scheme_digest, scheme, pack, pack_version, stacks) = match &current {
        // R2: the run picked nothing here now, and the border names where
        // it looked; an earlier run's pick still on the occasion is
        // overruled by the person's pick of nothing
        _ if named.is_none() && from_border.is_some() => from_border.expect("checked"),
        Some(r) => {
            let id = r.int(0)?;
            let parts: serde_json::Value = r
                .opt_text(5)?
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or(serde_json::Value::Null);
            let sql = format!(
                "SELECT stack_id FROM {} WHERE pick_id = {} ORDER BY stack_id",
                store.qualified("pick_stack"),
                d.param(1, Type::Int)
            );
            let stacks: Vec<i64> = store
                .query(&sql, &[Param::Int(id)])?
                .iter()
                .map(|x| x.int(0))
                .collect::<Result<_, _>>()?;
            (
                r.opt_text(1)?.unwrap_or_default().to_string(),
                r.opt_text(2)?.unwrap_or_default().to_string(),
                parts["scheme"].clone(),
                r.opt_text(3)?.unwrap_or_default().to_string(),
                r.opt_text(4)?.unwrap_or_default().to_string(),
                if named.is_some() { stacks } else { Vec::new() },
            )
        }
        None => {
            return Err(refused(format!(
                "review item {item} was raised before a border named its scheme and pack; run the picks again (nils pick run) and keep it then"
            )));
        }
    };
    // record 48, D1 of the move: no pick is written on a stack of a sample
    // sealed now
    let sealed = nils_registry::labels::sealed_for_writes(registry.store(), &stacks)
        .map_err(|e| refused(e.to_string()))?;
    if !sealed.is_empty() {
        return Err(refused(format!(
            "stack(s) {} are of a sample sealed for certification; no pick is written on a sealed stack until a certificate unseals it (record 48)",
            sealed
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let said = why.map(str::trim).filter(|w| !w.is_empty());
    let why = match (said, stacks.is_empty()) {
        (Some(w), _) => w.to_string(),
        (None, false) => format!("kept the run's pick ({})", borders.join(", ")),
        (None, true) => format!(
            "kept: no stack stands for the role {role} here ({})",
            borders.join(", ")
        ),
    };
    write_person(
        registry,
        &PersonRow {
            model: &model,
            role: &role,
            subject,
            day: &day,
            scheme_name: &scheme_name,
            scheme_digest: &scheme_digest,
            scheme: &scheme,
            pack: &pack,
            pack_version: &pack_version,
            stacks: &stacks,
            why: &why,
            actor,
            campaign: None,
            kept: Some(item),
        },
    )
}

/// What withdrawing a person's pick did.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PersonWithdrawn {
    pub id: i64,
    /// The run's picks that apply again.
    pub restored: Vec<i64>,
}

/// Withdraw a person's pick. The row stays and stops applying, and the
/// run's pick it overruled applies again. A run's pick is not withdrawn:
/// a person overrules it with a pick of their own.
pub fn withdraw_person(
    registry: &mut Registry,
    id: i64,
    actor: &str,
    why: Option<&str>,
) -> Result<PersonWithdrawn, PersonError> {
    use nils_registry::audit::{self, Action, Entry};

    let store = registry.store();
    let d = store.dialect();
    let sql = format!(
        "SELECT author_kind, withdrawn_at IS NOT NULL FROM {} WHERE id = {}",
        store.qualified("pick"),
        d.param(1, Type::Int)
    );
    let Some(row) = store.query_opt(&sql, &[Param::Int(id)])? else {
        return Err(refused(format!("no pick {id}")));
    };
    if row.text(0)? != "person" {
        return Err(refused(format!(
            "pick {id} is a run's; a person overrules it with a pick of their own (nils pick set), and the next run keeps that"
        )));
    }
    if row.int(1)? != 0 {
        return Err(refused(format!("pick {id} is already withdrawn")));
    }
    let now = now_iso();
    store.begin()?;
    let written = (|| -> Result<Vec<i64>, StoreError> {
        store.update_by_id(
            table("pick"),
            &[
                ("withdrawn_at", Param::from(now.as_str())),
                ("withdrawn_by", Param::from(actor)),
            ],
            "id",
            id,
        )?;
        let sql = format!(
            "SELECT id FROM {} WHERE overruled_by = {} ORDER BY id",
            store.qualified("pick"),
            d.param(1, Type::Int)
        );
        let restored: Vec<i64> = store
            .query(&sql, &[Param::Int(id)])?
            .iter()
            .map(|r| r.int(0))
            .collect::<Result<_, _>>()?;
        let sql = format!(
            "UPDATE {} SET withdrawn_at = NULL, overruled_by = NULL WHERE overruled_by = {}",
            store.qualified("pick"),
            d.param(1, Type::Int)
        );
        store.execute(&sql, &[Param::Int(id)])?;
        Ok(restored)
    })();
    let restored = match written {
        Ok(r) => r,
        Err(e) => {
            store.rollback().ok();
            return Err(e.into());
        }
    };
    store.commit()?;
    audit::record(
        registry,
        &Entry {
            principal: actor,
            action: Action::PickWithdraw,
            scope: serde_json::json!({ "pick": id, "restored": restored }),
            policy: None,
            job_id: None,
            details: why.map(|w| serde_json::json!({ "why": w })),
        },
    )?;
    Ok(PersonWithdrawn { id, restored })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nils_pack::pick::{Borders, Component, Family, Kind};

    fn row(stack: i64, pairs: &[(&str, &str)]) -> Row {
        Row {
            stack,
            subject: 1,
            study: 1,
            values: pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            roles: vec!["t1w".into()],
            scan: Scan::default(),
        }
    }

    fn model(family: Option<Family>) -> Model {
        Model {
            name: "main".into(),
            roles: vec!["t1w".into()],
            components: vec![Component {
                name: "slices".into(),
                weight: 1.0,
                kind: Kind::Percentile {
                    of: "n_instances".into(),
                    population: "slices".into(),
                    split_by: None,
                    missing: 0.4,
                    unknown: 0.6,
                },
            }],
            penalty: None,
            borders: Borders {
                runner_up_within: 0.05,
                ..Borders::default()
            },
            same_acquisition: vec![
                "technique".into(),
                "modifier".into(),
                "construct".into(),
                "echo_time".into(),
            ],
            families: family.into_iter().collect(),
            candidates: BTreeMap::new(),
            near_tie: Vec::new(),
        }
    }

    fn dixon() -> Family {
        Family {
            name: "dixon".into(),
            when: ("modifier".into(), "Dixon".into()),
            over: "construct".into(),
            ignoring: vec!["echo_time".into()],
            canonical: vec!["InPhase".into(), "Water".into()],
            apart_without_canonical: false,
            retake_above: 1,
        }
    }

    #[test]
    fn two_stacks_of_one_acquisition_are_one_candidate() {
        let m = model(None);
        let a = row(1, &[("technique", "MPRAGE"), ("echo_time", "2.3")]);
        let b = row(2, &[("technique", "MPRAGE"), ("echo_time", "2.3")]);
        let c = row(3, &[("technique", "MPRAGE"), ("echo_time", "4.6")]);
        let got = group(&m, &[&a, &b, &c]);
        assert_eq!(got.len(), 2, "the two echoes are two acquisitions");
        let together: Vec<&Vec<i64>> = got.iter().map(|c| &c.stacks).collect();
        assert!(together.contains(&&vec![1, 2]));
        assert!(together.contains(&&vec![3]));
    }

    #[test]
    fn the_outputs_of_one_dixon_do_not_compete_with_each_other() {
        // Four images of one acquisition. Without the family merge they are
        // four candidates for the session, and the pick is between them.
        let m = model(Some(dixon()));
        let of = |stack, construct, te| {
            row(
                stack,
                &[
                    ("technique", "VIBE"),
                    ("modifier", "Dixon"),
                    ("construct", construct),
                    ("echo_time", te),
                ],
            )
        };
        let w = of(1, "Water", "2.3");
        let f = of(2, "Fat", "2.4");
        let i = of(3, "InPhase", "2.5");
        let o = of(4, "OutPhase", "2.6");
        let got = group(&m, &[&w, &f, &i, &o]);
        assert_eq!(got.len(), 1, "one acquisition, one candidate");
        // And of its outputs only the canonical one, best first: v0's order is
        // in-phase then water, and a family is judged on what a reader would
        // actually measure on.
        assert_eq!(got[0].stacks, [3]);
    }

    #[test]
    fn a_family_with_nothing_worth_keeping_is_not_a_candidate() {
        // v0 drops it, and its comment says why: a Dixon with neither an
        // in-phase nor a water image is not a T1w anybody measures on.
        let m = model(Some(dixon()));
        let f = row(
            1,
            &[
                ("technique", "VIBE"),
                ("modifier", "Dixon"),
                ("construct", "Fat"),
                ("echo_time", "2.3"),
            ],
        );
        let o = row(
            2,
            &[
                ("technique", "VIBE"),
                ("modifier", "Dixon"),
                ("construct", "OutPhase"),
                ("echo_time", "2.4"),
            ],
        );
        assert!(group(&m, &[&f, &o]).is_empty());
    }

    #[test]
    fn a_lone_output_is_one_acquisition_either_way() {
        let m = model(Some(dixon()));
        let w = row(
            1,
            &[
                ("technique", "VIBE"),
                ("modifier", "Dixon"),
                ("construct", "Water"),
                ("echo_time", "2.3"),
            ],
        );
        let got = group(&m, &[&w]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].stacks, [1]);
    }

    fn mp2rage() -> Family {
        Family {
            name: "mp2rage".into(),
            when: ("technique".into(), "MP2RAGE".into()),
            over: "construct".into(),
            ignoring: vec!["echo_time".into()],
            canonical: vec!["UniformDenoised".into(), "Uniform".into()],
            apart_without_canonical: true,
            retake_above: 2,
        }
    }

    #[test]
    fn an_mp2rage_is_one_candidate_of_its_denoised_uniform_image() {
        // Record 51: v0's MP2RAGE preference, UniformDenoised and then
        // Uniform, which the pack before 0.17.0 did not carry. The two
        // inversions and the uniform image no longer compete with the
        // denoised one for the session.
        let mut m = model(Some(dixon()));
        m.families.push(mp2rage());
        let of = |stack, construct, te| {
            row(
                stack,
                &[
                    ("technique", "MP2RAGE"),
                    ("construct", construct),
                    ("echo_time", te),
                ],
            )
        };
        let a = of(1, "INV1", "2.9");
        let b = of(2, "INV2", "2.9");
        let c = of(3, "Uniform", "2.9");
        let d = of(4, "UniformDenoised", "2.9");
        let got = group(&m, &[&a, &b, &c, &d]);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].stacks, [4]);
        assert_eq!(got[0].family.as_deref(), Some("mp2rage"));
        // Without the denoised one, the uniform image.
        let got = group(&m, &[&a, &b, &c]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].stacks, [3]);
        // And with neither labelled, v0 "tags all": the acquisitions stand
        // apart as they were before the merge, and none is dropped.
        let got = group(&m, &[&a, &b]);
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got.iter().all(|c| c.family.is_none()));
    }

    #[test]
    fn a_candidate_keeps_each_stack_s_own_values_beside_the_merged_ones() {
        let m = model(None);
        let a = row(
            1,
            &[
                ("technique", "MPRAGE"),
                ("echo_time", "2.3"),
                ("n_instances", "176"),
            ],
        );
        let b = row(
            2,
            &[
                ("technique", "MPRAGE"),
                ("echo_time", "2.3"),
                ("n_instances", "40"),
            ],
        );
        let got = group(&m, &[&b, &a]);
        assert_eq!(got[0].stacks, [1, 2]);
        assert_eq!(got[0].each.len(), 2);
        assert_eq!(got[0].each[0]["n_instances"], "176");
        assert_eq!(got[0].each[1]["n_instances"], "40");
    }

    /// A row with what the fingerprint says of its acquisition: its images,
    /// its series and the number the scanner gave it, the moment it was
    /// acquired and whether its ImageType says DERIVED.
    fn scanned(
        mut r: Row,
        images: i64,
        (series, number): (i64, i64),
        moment: Option<&str>,
        derived: bool,
    ) -> Row {
        r.scan = Scan {
            images: Some(images),
            series,
            series_number: Some(number),
            acquired: moment.map(str::to_string),
            derived,
        };
        r
    }

    #[test]
    fn a_fragment_of_two_images_or_fewer_is_no_candidate() {
        // R6 of the 2026-10-10 borders study: a spin echo split into stacks
        // of 2, 19 and 1 images is one candidate of 19, and fragments alone
        // are no candidate at all.
        let m = model(None);
        let at = Some("2026-01-05 09:56:57.105000");
        let of = |stack, images, series| {
            scanned(
                row(stack, &[("technique", "SE"), ("echo_time", "15")]),
                images,
                (series, 8),
                at,
                false,
            )
        };
        let (a, b, c) = (of(1, 2, 10), of(2, 19, 11), of(3, 1, 12));
        let got = group(&m, &[&a, &b, &c]);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].stacks, [2]);
        assert!(group(&m, &[&a, &c]).is_empty());
    }

    #[test]
    fn one_moment_of_acquisition_is_one_scan_and_its_original_series_stays() {
        // R6, after record 38: two series of one acquisition time are one
        // acquisition stored twice, a second reconstruction or a re-send,
        // and the candidate keeps the scanner's original, the series it
        // wrote first.
        let m = model(None);
        let at = Some("2026-01-05 13:48:19.795000");
        let of = |stack, series: (i64, i64), moment, derived| {
            scanned(
                row(stack, &[("technique", "SPACE"), ("echo_time", "386")]),
                448,
                series,
                moment,
                derived,
            )
        };
        let first = of(1, (20, 11), at, false);
        let again = of(2, (21, 13), at, false);
        let got = group(&m, &[&again, &first]);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].stacks, [1], "the series written first");
        assert_eq!(got[0].acquired, [at.map(str::to_string)]);
        // An original image over a derived one, whatever their numbers.
        let derived = of(3, (22, 11), at, true);
        let original = of(4, (23, 13), at, false);
        assert_eq!(group(&m, &[&derived, &original])[0].stacks, [4]);
        // Two moments are two takes, both named, for the retake to judge.
        let later = of(5, (24, 15), Some("2026-01-05 14:02:00.000000"), false);
        let got = group(&m, &[&first, &later]);
        assert_eq!(got[0].stacks, [1, 5]);
        assert_eq!(got[0].acquired.len(), 2);
        // A moment nobody wrote down is never the same as another.
        let unknown = of(6, (25, 17), None, false);
        assert_eq!(group(&m, &[&first, &unknown])[0].stacks, [1, 6]);
        // And the stacks of one series are its parts, never a copy of each
        // other.
        let part = of(7, (20, 11), at, false);
        assert_eq!(group(&m, &[&first, &part])[0].stacks, [1, 7]);
    }

    #[test]
    fn a_stack_s_slice_count_is_its_images_where_its_files_hold_frames() {
        // R5 of the 2026-10-10 borders study: one enhanced file of 176
        // frames is a volume of 176 slices, not of one.
        let file = row(1, &[("n_instances", "1")]).with_scan(Scan {
            images: Some(176),
            ..Scan::default()
        });
        assert_eq!(file.values["n_instances"], "176");
        // A stack of single-frame images is what it was.
        let plain = row(2, &[("n_instances", "176")]).with_scan(Scan {
            images: Some(176),
            ..Scan::default()
        });
        assert_eq!(plain.values["n_instances"], "176");
        // And a model that reads no slice count is given none.
        let unread = row(3, &[("technique", "MPRAGE")]).with_scan(Scan {
            images: Some(176),
            ..Scan::default()
        });
        assert!(!unread.values.contains_key("n_instances"));
    }

    #[test]
    fn a_candidate_takes_each_number_at_its_largest() {
        // A bundle's slice count is the fullest volume in it, not whichever
        // one the database returned first.
        let m = model(None);
        let a = row(
            1,
            &[
                ("technique", "MPRAGE"),
                ("echo_time", "2.3"),
                ("n_instances", "40"),
            ],
        );
        let b = row(
            2,
            &[
                ("technique", "MPRAGE"),
                ("echo_time", "2.3"),
                ("n_instances", "176"),
            ],
        );
        let got = group(&m, &[&a, &b]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].get("n_instances"), "176");
    }

    #[test]
    fn a_population_is_what_the_candidates_of_that_role_are() {
        let m = model(None);
        let a = row(1, &[("technique", "MPRAGE"), ("n_instances", "176")]);
        let b = row(2, &[("technique", "MPRAGE"), ("n_instances", "160")]);
        let c = row(3, &[("technique", "TSE"), ("n_instances", "40")]);
        let r = build_reference(&m, "registry", &[&a, &b, &c]);
        assert_eq!(r.total, 3);
        assert!((r.share("technique", "MPRAGE") - 2.0 / 3.0).abs() < 1e-9);
        assert!((r.share("technique", "TSE") - 1.0 / 3.0).abs() < 1e-9);
        // Three values is the floor for a bucket, which is v0's and is kept:
        // a bucket drawn from two numbers says more about the two than about
        // the population.
        assert!(r.percentiles.contains_key("slices"));
        let two = build_reference(&m, "registry", &[&a, &b]);
        assert!(two.percentiles.is_empty());
    }

    #[test]
    fn a_scheme_is_named_on_the_row_because_it_decides_what_an_occasion_is() {
        let same_day = short_scheme(&Scheme::default());
        let fortnight = short_scheme(&Scheme {
            window_days: 14,
            ..Scheme::default()
        });
        assert_ne!(same_day, fortnight);
        assert!(fortnight.contains("14d"), "{fortnight}");
    }
}
