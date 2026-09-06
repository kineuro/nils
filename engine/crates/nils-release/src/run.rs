// SPDX-License-Identifier: AGPL-3.0-only

//! One release: what it selects, what it writes, and what it records
//! (`docs/specs/wave3-anonymize-and-bids.md`, §8).
//!
//! v0 has two exports. Its own runner says the two callers "run the same
//! underlying engine ... the two callers only differ in scope, output root,
//! and pipeline coupling", and all three differences are gone in v1: digest
//! replaced the cohort pipeline, a cohort is a membership fact rather than a
//! pipeline instance, and the root is an argument. So there is one.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::name;
use nils_registry::day::Day;
use nils_registry::schema::{Type, table};
use nils_registry::session::{self, Scheme};
use nils_registry::store::{Error as StoreError, Insert, Param, Store};
use nils_registry::{Registry, time::now_iso};

use crate::policy::{Policy, Uids};
use crate::scrub::{self, Plan};
use crate::tags::Category;
use crate::uid::Remap;

/// What a release will write.
///
/// **An enumeration, not a language.** Every field narrows, and an empty
/// selection is everything. What a study means by its cohort, an inclusion
/// rule, a diagnosis, a field strength, is a **query**, and the query AST is
/// the door for that; a release takes the answer. The grains below are the
/// four a cohort is actually made of, and each of them is something a person
/// or a query can hand over as a list:
///
/// - a set of subjects,
/// - a subject and one of its sessions,
/// - a stack, which already says whose session it is,
/// - and what a stack is, which is the pack's word rather than a study's.
///
/// v0 has the first three in its manifest resolver and the fourth in its
/// export config, in two callers with two shapes; here they are one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selection {
    /// Subject codes.
    pub subjects: Vec<String>,
    /// A subject and one of its sessions, by the label the scheme gives it:
    /// `("abc", "M06")`. Matched after the sessions are derived, because a
    /// session is derived and never stored (§5).
    pub sessions: Vec<(String, String)>,
    /// Stacks, by id. The narrowest grain, and the one a query returns.
    pub stacks: Vec<i64>,
    /// Stacks holding this value on the disposition axis. Empty means the
    /// release's default, which is everything that is not `excluded`.
    pub dispositions: Vec<String>,
    /// Stacks holding one of these roles.
    pub roles: Vec<String>,
    /// Only the stacks a pick chose (§10).
    pub picked_only: bool,
    pub modality: Option<String>,
    /// Wave 4a §8: stacks by what the pack says they are, as `(axis,
    /// value)` pairs; several values of one axis are alternatives, several
    /// axes all have to hold.
    pub axes: Vec<(String, String)>,
    /// Wave 4a §8: the cohorts the subjects came from, for the record; the
    /// members are already in `subjects`.
    pub cohorts: Vec<String>,
}

impl Selection {
    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "subjects": self.subjects,
            "sessions": self.sessions,
            "stacks": self.stacks,
            "dispositions": self.dispositions,
            "roles": self.roles,
            "picked_only": self.picked_only,
            "modality": self.modality,
            "axes": self.axes,
            "cohorts": self.cohorts,
        })
    }

    pub fn is_everything(&self) -> bool {
        self == &Selection::default()
    }
}

/// What a run says when it is done.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Report {
    pub release_id: i64,
    pub name: String,
    /// `YYYY.MM.DD.N` (§8.6).
    pub version: String,
    /// The version this one was worked out against, if there was one.
    pub previous: Option<String>,
    pub root: String,
    pub policy: String,
    pub subjects: i64,
    pub stacks: i64,
    /// Files in the tree at this version, which on a re-run is mostly files
    /// this run did not touch.
    pub files: i64,
    pub bytes: i64,
    /// Files this run actually wrote, which is what it cost.
    pub written: i64,
    /// Stacks by what became of them (§8.6). The first number is the point:
    /// a re-run after a QC decision should leave nearly everything alone.
    pub unchanged: i64,
    pub moved: i64,
    pub rewritten: i64,
    pub added: i64,
    pub removed: i64,
    /// Files a reader could not read or a writer could not write, by reason.
    pub refused: BTreeMap<String, i64>,
    /// Every change, by tag and action, without a value anywhere.
    pub changes: BTreeMap<String, i64>,
    /// Stacks held back because the file says their pixels carry text, and
    /// stacks the file said nothing about (§8.4). The second number is the one
    /// worth reading: "no tag" is not "no text".
    pub burned_in: i64,
    pub unjudged: i64,
    /// Which layout was written, and for BIDS what it chose (§9.3).
    pub layout: String,
    pub placements: BTreeMap<String, String>,
    /// The converter that was found, if one was needed (§9.6).
    pub converter: Option<String>,
    /// Stacks by the route of §9.3 they took.
    pub routes: BTreeMap<String, i64>,
    /// And, for the ones that went nowhere, why. Never a silent drop.
    pub nowhere: BTreeMap<String, i64>,
    /// Stacks written as DICOM because the converter would not convert them.
    /// v0 carries a hard-coded list of vendors instead, so a stack it could
    /// have converted is skipped and one it cannot is a failure.
    pub unconvertible: BTreeMap<String, i64>,
    /// Wave 4a §7.4: what the clinical export wrote, counted: subjects with
    /// a sex, with an age, sessions with an age, observations by kind.
    pub clinical: BTreeMap<String, i64>,
    /// Wave 4a §12: stacks the last version had written whose files were
    /// gone from the tree, written again rather than carried on the
    /// state's word.
    pub restored: i64,
    pub seconds: f64,
}

/// Which of the two layouts of §9 a release writes.
///
/// Neither is a fallback for the other. The descriptive one names every stack
/// in the archive; the BIDS one names the 47 percent the standard admits and
/// routes the rest, which is what makes it a dataset a validator passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    #[default]
    Descriptive,
    Bids,
}

impl Layout {
    pub fn name(self) -> &'static str {
        match self {
            Layout::Descriptive => "descriptive",
            Layout::Bids => "bids",
        }
    }

    pub fn parse(text: &str) -> Option<Layout> {
        match text {
            "descriptive" => Some(Layout::Descriptive),
            "bids" => Some(Layout::Bids),
            _ => None,
        }
    }
}

/// Where a stack's files go: a directory, and in BIDS the stem they share.
///
/// The two layouts differ in exactly this. In the descriptive one a stack owns
/// a directory, so its place is that directory. In BIDS stacks share `anat/`
/// and are told apart by their filenames, so its place is the directory and the
/// stem. Either way **the place is a prefix of every file's path**, which is
/// what lets §8.6 rename a stack without knowing which layout it is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub dir: String,
    pub stem: Option<String>,
}

impl Place {
    fn dir(dir: String) -> Place {
        Place { dir, stem: None }
    }

    fn file(dir: String, stem: String) -> Place {
        Place {
            dir,
            stem: Some(stem),
        }
    }

    /// The prefix every file of the stack shares.
    pub fn key(&self) -> String {
        match &self.stem {
            Some(stem) => format!("{}/{stem}", self.dir),
            None => self.dir.clone(),
        }
    }
}

/// What one release needs beyond its policy.
pub struct Settings<'a> {
    pub name: &'a str,
    pub root: &'a Path,
    pub policy: &'a Policy,
    pub categories: Vec<Category>,
    pub selection: Selection,
    pub scheme: &'a Scheme,
    /// The private elements the pack says are worth keeping (§8.4).
    pub private: &'a [nils_pack::private::Allowed],
    /// What to do with a stack whose file says nothing about its pixels.
    pub on_unknown: crate::burned::OnUnknown,
    pub actor: &'a str,
    /// The key the pseudonyms and the UID remapping are derived from.
    pub key: &'a [u8],
    /// The pack itself, for its BIDS mapping and its axis vocabularies (§9.2).
    pub pack: &'a nils_pack::pack::Pack,
    /// Which layout to write (§9).
    pub layout: Layout,
    /// The placements the release chose, for the two BIDS has no answer for
    /// (§9.3).
    pub places: crate::bids::place::Options,
    /// The converter, found before anything is written (§9.6). Required by the
    /// BIDS layout and unused by the descriptive one.
    pub converter: Option<&'a crate::bids::convert::Converter>,
    /// Whether the NIfTI is gzipped.
    /// Wave 4a §7.4: the kinds of observation whose nearest value each
    /// session carries in `_sessions.tsv`, by name; empty means the
    /// vocabulary's primary kinds.
    pub observations: &'a [String],
    pub compress: bool,
    /// Whose dataset it is (§9.5). Empty means the actor who ran the release,
    /// which is the honest default and not a claim about authorship.
    pub authors: &'a [String],
}

/// What went wrong.
#[derive(Debug)]
pub enum Error {
    Store(StoreError),
    Refused(String),
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Store(e) => write!(f, "{e}"),
            Error::Refused(m) => f.write_str(m),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<StoreError> for Error {
    fn from(e: StoreError) -> Error {
        Error::Store(e)
    }
}

/// One instance to write.
struct Instance {
    id: i64,
    stack: i64,
    study: i64,
    path: String,
    /// What the walker of Wave 1 recorded about the source file. A re-run
    /// decides a stack is unchanged partly from these, so it trusts exactly
    /// what the digest trusts and no more (§8.6).
    size: i64,
    mtime: i64,
}

/// What one stack's release is, once the plan and the state have been joined
/// (Wave 4a §4): where it goes, what decides its bytes, and what became of it.
struct Job {
    stack: i64,
    place: Place,
    /// Where it goes when the converter refuses it, which is the same place
    /// the planner would give it had it been routed to `sourcedata` in the
    /// first place. Computed by the planner so a fallback and a plan cannot
    /// disagree, which is what made every re-run move them.
    fallback: Place,
    /// Which of §9.3's routes it took. In the descriptive layout there is one.
    route: String,
    content: String,
    change: crate::version::Change,
    /// What the state said, when the stack was there.
    was: Option<State>,
    code: String,
    offset: crate::dates::Offset,
    /// Read only when the stack is written: a planned stack carries no
    /// instances, which is what keeps a version's memory a function of stacks
    /// and not of files.
    instances: Vec<Instance>,
}

/// The current state of one stack of a dataset, as `release_stack` holds it.
#[derive(Debug, Clone)]
struct State {
    content: String,
    place: Place,
    route: String,
    files: i64,
    bytes: i64,
    digest: String,
    /// A converted stack's files are its stem plus these; a DICOM stack owns
    /// its directory.
    extensions: Vec<String>,
}

impl State {
    /// The files this state describes, as far as the state knows them.
    fn files(&self) -> Files {
        match &self.place.stem {
            Some(stem) => Files::Named(
                self.extensions
                    .iter()
                    .map(|e| format!("{}/{stem}{e}", self.place.dir))
                    .collect(),
            ),
            None => Files::Directory(self.place.dir.clone()),
        }
    }
}

/// What a stack's files are on disk: a directory it owns, or named files.
enum Files {
    Directory(String),
    Named(Vec<String>),
}

/// What one version knows about the version before it.
struct Earlier {
    id: i64,
    version: String,
}

/// One file a version wrote.
#[derive(Debug, Clone)]
struct Wrote {
    path: String,
    digest: String,
    bytes: i64,
}

/// Run one release.
/// A release is a job (Wave 4a §9.1): claimed before anything is read,
/// finished with what happened, refused while another release is fresh.
pub fn run(registry: &mut Registry, settings: &Settings) -> Result<Report, Error> {
    use nils_registry::job;
    let job_id = job::claim(
        registry.store(),
        &job::Claim {
            kind: "release",
            name: settings.name,
            args: serde_json::json!({
                "layout": settings.layout.name(),
                "root": settings.root.display().to_string(),
                "selection": settings.selection.as_json(),
            }),
        },
    )
    .map_err(|e| match e {
        job::Error::Store(e) => Error::Store(e),
        other => Error::Refused(other.to_string()),
    })?;
    let result = run_release(registry, settings);
    let (state, error) = match &result {
        Ok(_) => (job::State::Done, None),
        Err(e) => (job::State::Failed, Some(e.to_string())),
    };
    let _ = job::finish(registry.store(), job_id, state, error.as_deref());
    // Wave 4a §9.2: the audit row, under the policy it ran under.
    if let Ok(report) = &result {
        nils_registry::audit::record(
            registry,
            &nils_registry::audit::Entry {
                principal: settings.actor,
                action: nils_registry::audit::Action::Release,
                scope: serde_json::json!({
                    "release": settings.name,
                    "version": report.version,
                    "layout": settings.layout.name(),
                    "subjects": report.subjects,
                    "files": report.files,
                }),
                policy: Some(settings.policy.as_json()),
                job_id: Some(job_id),
                details: Some(settings.selection.as_json()),
            },
        )
        .map_err(Error::Store)?;
    }
    result
}

fn run_release(registry: &mut Registry, settings: &Settings) -> Result<Report, Error> {
    let started = std::time::Instant::now();
    // §4.3, before anything is read: a release that shifts dates and preserves
    // UIDs has shifted nothing, and a warning is read after the tree exists.
    settings
        .policy
        .check()
        .map_err(|e| Error::Refused(e.to_string()))?;
    // And the other half of §4.3: a session label that is a date under a
    // policy that moves dates would put the true date back in the path.
    if settings.policy.dates.moves_dates() && settings.scheme.naming == session::Naming::Date {
        return Err(Error::Refused(format!(
            "dates {} and a session scheme that labels by the date is not a policy: the tree \
             would carry the date the files no longer do (§4.3). Use a months or ordinal \
             scheme, or keep the dates.",
            settings.policy.dates.name()
        )));
    }

    // §9.2 and §9.6, before a registry row exists. A pack with no mapping
    // cannot name a BIDS tree, and a converter is not a thing to discover
    // halfway through an archive.
    if settings.layout == Layout::Bids {
        if settings.pack.bids.is_empty() {
            return Err(Error::Refused(format!(
                "the {} pack declares no BIDS mapping, so it cannot say which of its values \
                 means T1w (§9.2). Release the descriptive layout, or give the pack a \
                 bids.yml.",
                settings.pack.name
            )));
        }
        if settings.converter.is_none() {
            return Err(Error::Refused(
                "a BIDS tree is NIfTI and no converter was found (§9.6)".to_string(),
            ));
        }
    }

    // Wave 4a §7.4, before anything is planned: the observation kinds the
    // release names must exist and must not be sensitive, whatever the
    // layout.
    observation_kinds(registry.store(), settings)?;

    // §9.6, before a byte is written: the root has to be writable and there
    // has to be room. A release that discovers a full disk after 400 GB has
    // written 400 GB for nothing, and what it reports is the operating
    // system's word for it rather than what to do about it.
    let wanted = selection_bytes(registry.store(), &settings.selection)?;
    preflight(settings.root, wanted)?;
    let days = study_days(registry.store())?;
    let pixels = pixel_verdicts(registry.store())?;
    let named = places(registry.store(), &days, settings.scheme, settings.pack)?;
    // The dataset this is a version of, and the version before it, read before
    // anything is written.
    let dataset = dataset_of(registry.store(), settings.name, settings.root)?;
    let earlier = newest_version(registry.store(), dataset)?;
    let version = crate::version::next(today(), earlier.as_ref().map(|p| p.version.as_str()));

    let mut placements: BTreeMap<String, String> = BTreeMap::new();
    if settings.layout == Layout::Bids {
        placements.insert(
            "localizers".to_string(),
            settings.places.localizers.name().to_string(),
        );
        placements.insert(
            "synthetic".to_string(),
            settings.places.synthetic.name().to_string(),
        );
    }
    let mut report = Report {
        name: settings.name.to_string(),
        layout: settings.layout.name().to_string(),
        placements,
        converter: settings.converter.map(|c| c.describe()),
        version: version.clone(),
        previous: earlier.as_ref().map(|p| p.version.clone()),
        root: settings.root.display().to_string(),
        policy: settings.policy.describe(),
        ..Report::default()
    };
    let remap = match settings.policy.uids {
        Uids::Remap => Some(Remap::new(settings.policy.root.clone(), settings.key)),
        Uids::Preserve => None,
    };
    report.release_id = open_row(
        registry.store(),
        settings,
        &version,
        dataset,
        earlier.as_ref(),
        &report.placements,
    )?;

    // The parts of a stack's content digest that are the same for every stack
    // in the release (§8.6).
    let categories: String = settings
        .categories
        .iter()
        .map(|c| c.name())
        .collect::<Vec<_>>()
        .join(",");
    let private: String = settings
        .private
        .iter()
        .map(|a| a.text())
        .collect::<Vec<_>>()
        .join(",");
    let pack = format!("{}@{}", settings.pack.name, settings.pack.version);

    // §9.4: the time each stack was acquired, under the date policy, for the
    // standard's own columns. Computed once, before anything is written.
    let acq_times = match settings.layout {
        Layout::Bids => acquisition_times(registry.store(), &days)?,
        Layout::Descriptive => HashMap::new(),
    };

    // ---------------------------------------------------------------- plan
    //
    // What each stack is and where it goes, worked out **one subject at a
    // time** and written to `release_plan` as it is decided. Nothing here reads
    // or writes a byte of the tree, and nothing here holds more than one
    // subject's instances: the plan is what makes a re-run cheap, so it is
    // taken from the registry alone, and its memory is a function of the
    // largest subject and not of the archive (Wave 4a §4).
    let mut held: std::collections::HashSet<i64> = std::collections::HashSet::new();
    // What went nowhere, with the reason (§9.3).
    let mut absent: Vec<(i64, String, String)> = Vec::new();
    let mut planned: Vec<Vec<Param>> = Vec::new();
    let mut people: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Wave 4a §7.4: each subject's id, and each session's earliest study day
    // and the subject's offset, for the clinical export.
    let mut subjects_seen: BTreeMap<String, i64> = BTreeMap::new();
    let mut sessions_seen: BTreeMap<(String, String), (i64, Day, crate::dates::Offset)> =
        BTreeMap::new();
    let mut stacks_planned = 0i64;
    for (subject, code) in selected_subjects(registry.store(), &settings.selection)? {
        let mine = select_subject(registry.store(), &settings.selection, subject)?;
        if mine.is_empty() {
            continue;
        }
        let labels = session_labels(&mine, &days, settings.scheme);
        let offset = match settings.policy.dates {
            crate::dates::Policy::Shift => {
                let o = crate::dates::draw(settings.key, subject);
                remember_offset(registry, subject, o)?;
                o
            }
            _ => crate::dates::Offset(0),
        };
        let mut grouped: BTreeMap<i64, Vec<Instance>> = BTreeMap::new();
        for i in mine {
            grouped.entry(i.stack).or_default().push(i);
        }
        for (stack, instances) in grouped {
            let study = instances[0].study;
            let label = labels
                .get(&study)
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());
            // A subject and one of its sessions, matched here rather than in
            // SQL because a session is derived from a scheme on read and is
            // never a column (§5).
            if !settings.selection.sessions.is_empty()
                && !settings
                    .selection
                    .sessions
                    .iter()
                    .any(|(who, which)| *who == code && *which == label)
            {
                continue;
            }
            // §8.4. The engine does not look at pixels; it reads what the file
            // says about them, and holds what the file will not say. A held
            // stack is simply not in this version, so one an earlier version
            // wrote is removed from the tree rather than left in it.
            match pixels
                .get(&stack)
                .copied()
                .unwrap_or(crate::burned::Verdict::Unknown)
            {
                crate::burned::Verdict::Burned => {
                    held.insert(stack);
                    report.burned_in += 1;
                    continue;
                }
                crate::burned::Verdict::Unknown
                    if settings.on_unknown == crate::burned::OnUnknown::Hold =>
                {
                    held.insert(stack);
                    report.unjudged += 1;
                    continue;
                }
                _ => {}
            }
            let placed = named.get(&stack);
            // §9.1. A stack with no name is one nothing classified, which is a
            // stack the release should not silently rename into something
            // readable.
            let (folder, stem) = match placed {
                Some(p) => (p.descriptive.folder.clone(), p.descriptive.name.clone()),
                None => ("misc".to_string(), format!("stack-{stack:08}")),
            };
            let route = match settings.layout {
                Layout::Descriptive => crate::bids::place::Route::Raw,
                Layout::Bids => crate::bids::place::route(
                    placed.and_then(|p| p.disposition.as_deref()),
                    placed.is_some_and(|p| p.synthetic),
                    &placed
                        .map(|p| p.bids.clone())
                        .unwrap_or(Err(crate::bids::name::Why::NoSuffix)),
                    settings.places,
                ),
            };
            // §9.3's fourth route. Never a silent drop: the stack is out of
            // this version, so one an earlier version wrote is removed from
            // the tree, and the reason is recorded and reported.
            if let crate::bids::place::Route::Nowhere(why) = &route {
                *report.nowhere.entry(why.kind().to_string()).or_insert(0) += 1;
                *report.routes.entry("nowhere".to_string()).or_insert(0) += 1;
                absent.push((stack, why.kind().to_string(), why.to_string()));
                continue;
            }
            let content = crate::version::content_of(
                &subject_policy(settings, &code, offset),
                &categories,
                &private,
                &pack,
                &instances
                    .iter()
                    .map(|i| (i.id, i.size, i.mtime))
                    .collect::<Vec<_>>(),
            );
            let place = place_of(&route, settings, &code, &label, &folder, &stem, placed);
            let fallback = place_of(
                &crate::bids::place::Route::SourceData,
                settings,
                &code,
                &label,
                &folder,
                &stem,
                placed,
            );
            people.insert(code.clone());
            subjects_seen.insert(code.clone(), subject);
            if let Some(day) = days.get(&study).copied() {
                sessions_seen
                    .entry((code.clone(), label.clone()))
                    .and_modify(|(_, d, _)| {
                        if day < *d {
                            *d = day;
                        }
                    })
                    .or_insert((subject, day, offset));
            }
            stacks_planned += 1;
            planned.push(vec![
                Param::Int(report.release_id),
                Param::Int(stack),
                Param::from(content),
                Param::from(place.dir),
                match place.stem {
                    Some(stem) => Param::from(stem),
                    None => Param::Null,
                },
                Param::from(route.name()),
                Param::from(fallback.dir),
                match fallback.stem {
                    Some(stem) => Param::from(stem),
                    None => Param::Null,
                },
                Param::from(code.as_str()),
                Param::from(label),
                Param::Int(offset.0),
            ]);
            if planned.len() >= PLAN_BATCH {
                write_plan(registry.store(), &planned)?;
                planned.clear();
            }
        }
    }
    write_plan(registry.store(), &planned)?;
    planned.clear();
    report.stacks = stacks_planned;
    report.subjects = people.len() as i64;

    // ---------------------------------------------------------------- diff
    //
    // The five outcomes of §8.6, as a join of the plan against the state,
    // paged, so that what is held here is one row per planned stack and never
    // a row per file.
    let mut jobs: Vec<Job> = Vec::new();
    let mut after = 0i64;
    loop {
        let page = plan_page(registry.store(), dataset, report.release_id, after)?;
        if page.is_empty() {
            break;
        }
        for (planned, was) in page {
            after = planned.stack;
            let mut route = planned.route.clone();
            let mut place = planned.place.clone();
            // A conversion the converter refused last time will refuse it
            // again, because nothing it depends on has changed: the converter
            // is in the content digest and so is everything about the stack.
            // So the fallback is **planned** rather than retried, and the
            // stack is left alone. Without this, every re-run rewrites every
            // stack that ever fell back, which is the incremental promise
            // broken for exactly the stacks that cost the most.
            let fell_back = was
                .as_ref()
                .is_some_and(|w| w.content == planned.content && w.route == "sourcedata");
            if fell_back && route != "sourcedata" {
                route = "sourcedata".to_string();
                place = planned.fallback.clone();
            }
            *report.routes.entry(route.clone()).or_insert(0) += 1;
            let key = place.key();
            let mut change = crate::version::compare(
                was.as_ref()
                    .map(|w| crate::version::Was {
                        content: w.content.clone(),
                        dir: w.place.key(),
                    })
                    .as_ref(),
                &planned.content,
                &key,
            );
            // A route that changed changed the bytes: a stack converted last
            // time and written as DICOM this time is not the same file under
            // a new name.
            if change == crate::version::Change::Moved
                && was.as_ref().is_some_and(|w| w.route != route)
            {
                change = crate::version::Change::Rewritten;
            }
            // A stack whose files the last version did not write is not one
            // this version may carry forward, whatever the digest says: the
            // digest describes the decision and the state describes the tree,
            // and only the state knows a file was refused.
            let carried = !change.is_work() || change == crate::version::Change::Moved;
            if carried && !was.as_ref().is_some_and(|w| w.files > 0) {
                change = crate::version::Change::Rewritten;
            }
            // Wave 4a §12, bar 9 found this: the state describes the tree as
            // it was written, and a file somebody removed from the tree
            // since is not carried forward on the state's word. A stack
            // whose place is gone from the disk is written again, and the
            // report says how many were.
            if let (true, Some(w)) = (
                !change.is_work() || change == crate::version::Change::Moved,
                was.as_ref(),
            ) && !place_on_disk(settings.root, &w.place)
            {
                change = crate::version::Change::Rewritten;
                report.restored += 1;
            }
            jobs.push(Job {
                stack: planned.stack,
                place,
                fallback: planned.fallback,
                route,
                content: planned.content,
                change,
                was,
                code: planned.code,
                offset: planned.offset,
                instances: Vec::new(),
            });
        }
    }
    // What the state holds and this version does not.
    let gone = gone_stacks(registry.store(), dataset, report.release_id)?;

    // ---------------------------------------------------------------- apply
    //
    // Everything that leaves goes first, so that a name a move is about to
    // take is free by the time the move happens.
    for (_, state) in &gone {
        drop_files(settings.root, &state.files());
    }
    for job in &jobs {
        if job.change == crate::version::Change::Rewritten
            && let Some(was) = &job.was
        {
            drop_files(settings.root, &was.files());
        }
    }
    let moved = move_them(settings.root, &jobs);

    // And only now is anything written.
    let mut rows: Vec<Vec<Param>> = Vec::new();
    let mut moves: Vec<Vec<Param>> = Vec::new();
    let mut scans: BTreeMap<(String, String), Vec<crate::bids::dataset::Scan>> = BTreeMap::new();
    for job in &mut jobs {
        // A move whose source is not where the last version left it is not a
        // move. Somebody emptied the tree, and the stack is written again.
        if job.change == crate::version::Change::Moved && !moved.contains(&job.stack) {
            job.change = crate::version::Change::Rewritten;
            if let Some(was) = &job.was {
                drop_files(settings.root, &was.files());
            }
        }
        let state: State = match job.change {
            crate::version::Change::Unchanged | crate::version::Change::Moved => {
                // The bytes are the ones the last version wrote, and the
                // digest with them: nothing was read, so nothing is recomputed.
                let was = job.was.clone().unwrap_or_else(|| State {
                    content: job.content.clone(),
                    place: job.place.clone(),
                    route: job.route.clone(),
                    files: 0,
                    bytes: 0,
                    digest: String::new(),
                    extensions: Vec::new(),
                });
                State {
                    place: job.place.clone(),
                    ..was
                }
            }
            _ => {
                job.instances = instances_of(registry.store(), job.stack)?;
                let code = job.code.clone();
                let plan = Plan {
                    policy: settings.policy,
                    categories: &settings.categories,
                    private: settings.private,
                    code: &code,
                    offset: job.offset,
                    remap: remap.as_ref(),
                };
                let written = match settings.layout {
                    Layout::Descriptive => {
                        let dir = job.place.dir.clone();
                        write_dicom(job, &plan, settings.root, &dir)
                    }
                    Layout::Bids => write_bids(job, &plan, settings, &mut report),
                };
                let mut wrote: Vec<Wrote> = Vec::new();
                for w in written {
                    match w {
                        Ok(w) => {
                            report.written += 1;
                            for ((tag, action), n) in &w.applied.changes {
                                *report.changes.entry(format!("{tag} {action}")).or_insert(0) += n;
                            }
                            wrote.push(w.wrote);
                        }
                        Err(why) => {
                            // By the reason and never by the path: a report
                            // that names the files it could not read names the
                            // source tree, which is the one thing a released
                            // dataset must not carry.
                            *report.refused.entry(why).or_insert(0) += 1;
                        }
                    }
                }
                job.instances.clear();
                roll_up(job, &wrote)
            }
        };
        report.files += state.files;
        report.bytes += state.bytes;
        rows.push(vec![
            Param::Int(dataset),
            Param::Int(job.stack),
            Param::Int(report.release_id),
            Param::from(state.content.as_str()),
            Param::from(state.place.dir.as_str()),
            match &state.place.stem {
                Some(stem) => Param::from(stem.as_str()),
                None => Param::Null,
            },
            Param::from(state.route.as_str()),
            Param::Int(state.files),
            Param::Int(state.bytes),
            Param::from(state.digest.as_str()),
            match state.extensions.is_empty() {
                true => Param::Null,
                false => Param::from(state.extensions.join(",")),
            },
        ]);
        match job.change {
            crate::version::Change::Unchanged => report.unchanged += 1,
            crate::version::Change::Moved => report.moved += 1,
            crate::version::Change::Rewritten => report.rewritten += 1,
            crate::version::Change::Added => report.added += 1,
            crate::version::Change::Removed => {}
        }
        if job.change.is_work() {
            moves.push(vec![
                Param::Int(report.release_id),
                Param::Int(job.stack),
                Param::from(job.change.name()),
                match &job.was {
                    Some(was) => Param::from(was.place.key()),
                    None => Param::Null,
                },
                Param::from(state.place.key()),
            ]);
        }
        // §9.4: the time in the standard's own slot, per image, so anything
        // joining on a date reads a column instead of parsing a directory name.
        if settings.layout == Layout::Bids
            && let Some(stem) = &state.place.stem
            && let Some(image) = state
                .extensions
                .iter()
                .find(|e| *e == ".nii.gz" || *e == ".nii")
        {
            let prefix = format!("sub-{}/", job.code);
            let path = format!("{}/{stem}{image}", state.place.dir);
            if let Some(rest) = path.strip_prefix(&prefix)
                && let Some((session, file)) = rest.split_once('/')
                && let Some(label) = session.strip_prefix("ses-")
            {
                scans
                    .entry((job.code.clone(), label.to_string()))
                    .or_default()
                    .push(crate::bids::dataset::Scan {
                        filename: file.to_string(),
                        acq_time: acq_times
                            .get(&job.stack)
                            .and_then(|t| under_policy(t, settings, job.offset)),
                    });
            }
        }
        if rows.len() >= PLAN_BATCH {
            upsert_state(registry.store(), &rows)?;
            rows.clear();
        }
    }
    upsert_state(registry.store(), &rows)?;
    for (stack, state) in &gone {
        report.removed += 1;
        moves.push(vec![
            Param::Int(report.release_id),
            Param::Int(*stack),
            Param::from(crate::version::Change::Removed.name()),
            Param::from(state.place.key()),
            Param::Null,
        ]);
    }
    forget_stacks(registry.store(), dataset, &gone)?;

    // §9.5. The files that make the tree a dataset rather than a pile of
    // correctly named images. v0 writes none of them.
    if settings.layout == Layout::Bids {
        let mut codes: Vec<String> = people.into_iter().collect();
        codes.sort();
        let clinical = clinical_rows(
            registry.store(),
            settings,
            &subjects_seen,
            &sessions_seen,
            &mut report,
        )?;
        write_dataset(settings, &report, &scans, &codes, &clinical).map_err(Error::Io)?;
    }

    write_rows(
        registry.store(),
        "release_move",
        &["release_id", "stack_id", "action", "was", "now"],
        &moves,
    )?;
    write_rows(
        registry.store(),
        "release_absent",
        &["release_id", "stack_id", "kind", "why"],
        &absent
            .iter()
            .map(|(stack, kind, why)| {
                vec![
                    Param::Int(report.release_id),
                    Param::Int(*stack),
                    Param::from(kind.as_str()),
                    Param::from(why.as_str()),
                ]
            })
            .collect::<Vec<_>>(),
    )?;
    write_changes(registry.store(), &report)?;
    if !held.is_empty() {
        raise_review(registry.store(), &report, &held, &pixels)?;
    }
    // §9.2. `func` requires `task` and no rule can invent one, so a person is
    // asked, **per study**: what the subject was doing is a property of the
    // occasion and not of a stack, and one answer settles every functional
    // stack of it.
    ask_about_tasks(registry.store(), &report, &absent, settings)?;
    forget_plan(registry.store(), report.release_id)?;
    close_row(registry.store(), &report)?;
    report.seconds = started.elapsed().as_secs_f64();
    Ok(report)
}

/// How many plan rows and state rows are written at once.
const PLAN_BATCH: usize = 2_000;

/// One row of the plan, as read back joined to the state.
struct Planned {
    stack: i64,
    content: String,
    place: Place,
    fallback: Place,
    route: String,
    code: String,
    offset: crate::dates::Offset,
}

fn write_plan(store: &mut Store, rows: &[Vec<Param>]) -> Result<(), Error> {
    write_rows(
        store,
        "release_plan",
        &[
            "release_id",
            "stack_id",
            "content",
            "dir",
            "stem",
            "route",
            "fallback_dir",
            "fallback_stem",
            "code",
            "label",
            "offset_days",
        ],
        rows,
    )
}

/// One page of the plan, joined to the state, after a stack id.
///
/// Paged so that a version holds one page of stacks at a time while it
/// decides, and never the archive.
fn plan_page(
    store: &mut Store,
    dataset: i64,
    release: i64,
    after: i64,
) -> Result<Vec<(Planned, Option<State>)>, Error> {
    let d = store.dialect();
    let sql = format!(
        "SELECT p.stack_id, p.content, p.dir, p.stem, p.route, p.fallback_dir, p.fallback_stem, \
                p.code, p.label, p.offset_days, \
                s.content, s.dir, s.stem, s.route, s.files, s.bytes, s.digest, s.extensions \
         FROM {} p LEFT JOIN {} s ON s.dataset_id = {} AND s.stack_id = p.stack_id \
         WHERE p.release_id = {} AND p.stack_id > {} \
         ORDER BY p.stack_id LIMIT {}",
        store.qualified("release_plan"),
        store.qualified("release_stack"),
        d.param(1, Type::Int),
        d.param(2, Type::Int),
        d.param(3, Type::Int),
        PLAN_BATCH,
    );
    let mut out = Vec::new();
    for r in store.query(
        &sql,
        &[Param::Int(dataset), Param::Int(release), Param::Int(after)],
    )? {
        let planned = Planned {
            stack: r.int(0)?,
            content: r.text(1)?.to_string(),
            place: Place {
                dir: r.text(2)?.to_string(),
                stem: r.opt_text(3)?.map(str::to_string),
            },
            route: r.text(4)?.to_string(),
            fallback: Place {
                dir: r.text(5)?.to_string(),
                stem: r.opt_text(6)?.map(str::to_string),
            },
            code: r.text(7)?.to_string(),
            offset: crate::dates::Offset(r.int(9)?),
        };
        let was = match r.opt_text(10)? {
            None => None,
            Some(content) => Some(State {
                content: content.to_string(),
                place: Place {
                    dir: r.text(11)?.to_string(),
                    stem: r.opt_text(12)?.map(str::to_string),
                },
                route: r.text(13)?.to_string(),
                files: r.int(14)?,
                bytes: r.int(15)?,
                digest: r.text(16)?.to_string(),
                extensions: r
                    .opt_text(17)?
                    .map(|e| e.split(',').map(str::to_string).collect())
                    .unwrap_or_default(),
            }),
        };
        out.push((planned, was));
    }
    Ok(out)
}

/// The stacks the state holds and the plan does not.
fn gone_stacks(store: &mut Store, dataset: i64, release: i64) -> Result<Vec<(i64, State)>, Error> {
    let d = store.dialect();
    let sql = format!(
        "SELECT s.stack_id, s.content, s.dir, s.stem, s.route, s.files, s.bytes, s.digest, \
                s.extensions \
         FROM {} s WHERE s.dataset_id = {} AND NOT EXISTS \
           (SELECT 1 FROM {} p WHERE p.release_id = {} AND p.stack_id = s.stack_id) \
         ORDER BY s.stack_id",
        store.qualified("release_stack"),
        d.param(1, Type::Int),
        store.qualified("release_plan"),
        d.param(2, Type::Int),
    );
    let mut out = Vec::new();
    for r in store.query(&sql, &[Param::Int(dataset), Param::Int(release)])? {
        out.push((
            r.int(0)?,
            State {
                content: r.text(1)?.to_string(),
                place: Place {
                    dir: r.text(2)?.to_string(),
                    stem: r.opt_text(3)?.map(str::to_string),
                },
                route: r.text(4)?.to_string(),
                files: r.int(5)?,
                bytes: r.int(6)?,
                digest: r.text(7)?.to_string(),
                extensions: r
                    .opt_text(8)?
                    .map(|e| e.split(',').map(str::to_string).collect())
                    .unwrap_or_default(),
            },
        ));
    }
    Ok(out)
}

/// Write the state: a new stack is inserted, a known one is overwritten.
fn upsert_state(store: &mut Store, rows: &[Vec<Param>]) -> Result<(), Error> {
    if rows.is_empty() {
        return Ok(());
    }
    let columns = [
        "dataset_id",
        "stack_id",
        "release_id",
        "content",
        "dir",
        "stem",
        "route",
        "files",
        "bytes",
        "digest",
        "extensions",
    ];
    store.begin()?;
    let result = store.insert(
        &Insert::new(table("release_stack"), &columns).on_conflict(
            nils_registry::dialect::Conflict::Update {
                target: &["dataset_id", "stack_id"],
                set: &columns[2..],
            },
        ),
        rows,
    );
    match result {
        Ok(_) => {
            store.commit()?;
            Ok(())
        }
        Err(e) => {
            store.rollback().ok();
            Err(Error::Store(e))
        }
    }
}

fn forget_stacks(store: &mut Store, dataset: i64, gone: &[(i64, State)]) -> Result<(), Error> {
    for chunk in gone.chunks(500) {
        let ids = chunk
            .iter()
            .map(|(id, _)| id.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "DELETE FROM {} WHERE dataset_id = {} AND stack_id IN ({ids})",
            store.qualified("release_stack"),
            store.dialect().param(1, Type::Int),
        );
        store.execute(&sql, &[Param::Int(dataset)])?;
    }
    Ok(())
}

fn forget_plan(store: &mut Store, release: i64) -> Result<(), Error> {
    let sql = format!(
        "DELETE FROM {} WHERE release_id = {}",
        store.qualified("release_plan"),
        store.dialect().param(1, Type::Int),
    );
    store.execute(&sql, &[Param::Int(release)])?;
    Ok(())
}

/// What a version knows about one stack once it has written it.
///
/// One digest over the files' digests, sorted by path, so that the same files
/// give the same roll-up whatever order they were written in.
fn roll_up(job: &Job, wrote: &[Wrote]) -> State {
    use blake2::Digest;
    let mut sorted: Vec<&Wrote> = wrote.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let mut h = blake2::Blake2s256::new();
    for w in &sorted {
        h.update(w.digest.as_bytes());
        h.update(b"\n");
    }
    let mut extensions: Vec<String> = Vec::new();
    if let Some(stem) = &job.place.stem {
        let prefix = format!("{}/{stem}", job.place.dir);
        for w in &sorted {
            if let Some(ext) = w.path.strip_prefix(&prefix)
                && !extensions.iter().any(|e| e == ext)
            {
                extensions.push(ext.to_string());
            }
        }
    }
    State {
        content: job.content.clone(),
        place: job.place.clone(),
        route: job.route.clone(),
        files: wrote.len() as i64,
        bytes: wrote.iter().map(|w| w.bytes).sum(),
        digest: match wrote.is_empty() {
            true => String::new(),
            false => hex::encode(h.finalize()),
        },
        extensions,
    }
}

/// The files that make a tree a dataset (§9.4 and §9.5).
///
/// v0 writes none of them, which is why its tree is not a dataset rather than
/// an invalid one.
/// Whether a stack's files, as the last version placed them, are still on
/// the disk: the directory exists, and in a layout where stacks share a
/// directory, at least one file there carries the stack's stem.
fn place_on_disk(root: &Path, place: &Place) -> bool {
    let dir = root.join(&place.dir);
    match &place.stem {
        None => dir.is_dir(),
        Some(stem) => std::fs::read_dir(&dir).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with(stem.as_str()))
        }),
    }
}

/// What the clinical layer lets into the tree (Wave 4a §7.4), under the
/// policy: per subject the sex and the age at the first session, per session
/// the age and the nearest observation of each kind the release names.
struct Clinical {
    participants: BTreeMap<String, BTreeMap<String, String>>,
    sessions: BTreeMap<(String, String), BTreeMap<String, String>>,
}

/// Wave 4a §7.4. An age is computed before the birth date goes (Wave 3
/// §8.3) and is written under every policy, because a number of years is
/// not a date; an observation's date moves with the subject's offset under
/// `shift` and is not written at all under `year`; the signed distance in
/// days from the session to the observation is written under every policy,
/// because it is what a join on the nearest value needs and it names no day.
fn clinical_rows(
    store: &mut Store,
    settings: &Settings,
    subjects: &BTreeMap<String, i64>,
    sessions: &BTreeMap<(String, String), (i64, Day, crate::dates::Offset)>,
    report: &mut Report,
) -> Result<Clinical, Error> {
    use nils_registry::clinical;
    let mut out = Clinical {
        participants: BTreeMap::new(),
        sessions: BTreeMap::new(),
    };
    let kinds = observation_kinds(store, settings)?;
    // Each subject's demographics, in one read.
    let mut demographics: HashMap<i64, (Option<Day>, Option<String>)> = HashMap::new();
    if !subjects.is_empty() {
        let t = table("subject");
        let born = store
            .dialect()
            .text_of(t.column("birth_date").expect("subject.birth_date"));
        let sql = format!(
            "SELECT id, {born}, sex FROM {} WHERE id IN ({})",
            store.qualified("subject"),
            subjects
                .values()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        for r in store.query(&sql, &[])? {
            demographics.insert(
                r.int(0)?,
                (
                    r.opt_text(1)?.and_then(|t| Day::parse(&t.replace('-', ""))),
                    r.opt_text(2)?.map(str::to_string),
                ),
            );
        }
    }
    // The first session per subject, for the age at entry.
    let mut first: BTreeMap<String, Day> = BTreeMap::new();
    for ((code, _), (_, day, _)) in sessions {
        first
            .entry(code.clone())
            .and_modify(|f| {
                if *day < *f {
                    *f = *day;
                }
            })
            .or_insert(*day);
    }
    for (code, id) in subjects {
        let mut row = BTreeMap::new();
        if let Some((born, sex)) = demographics.get(id) {
            if let Some(sex) = sex
                && !sex.is_empty()
            {
                row.insert("sex".to_string(), sex.clone());
                *report.clinical.entry("sex".to_string()).or_insert(0) += 1;
            }
            if let (Some(born), Some(at)) = (born, first.get(code))
                && let Some(age) = crate::dates::age_years(*born, *at)
            {
                row.insert("age".to_string(), age.to_string());
                *report.clinical.entry("age".to_string()).or_insert(0) += 1;
            }
        }
        out.participants.insert(code.clone(), row);
    }
    for ((code, label), (subject, day, offset)) in sessions {
        let mut row = BTreeMap::new();
        if let Some((Some(born), _)) = demographics.get(subject)
            && let Some(age) = crate::dates::age_years(*born, *day)
        {
            row.insert("age".to_string(), age.to_string());
            *report
                .clinical
                .entry("session age".to_string())
                .or_insert(0) += 1;
        }
        for kind in &kinds {
            let Some(near) = clinical::nearest(store, *subject, kind.id, *day)? else {
                continue;
            };
            let column = column_of(&kind.name);
            let value = match (near.number, &near.value) {
                (Some(n), _) => format!("{n}"),
                (None, Some(v)) => v.clone(),
                (None, None) => "yes".to_string(),
            };
            row.insert(column.clone(), value);
            row.insert(format!("{column}_days"), near.offset_days.to_string());
            match settings.policy.dates {
                crate::dates::Policy::Year => {}
                policy => {
                    let when = crate::dates::apply(policy, *offset, near.date);
                    row.insert(format!("{column}_date"), when.to_string());
                }
            }
            *report
                .clinical
                .entry(format!("nearest {}", kind.name))
                .or_insert(0) += 1;
        }
        out.sessions.insert((code.clone(), label.clone()), row);
    }
    Ok(out)
}

/// The kinds the release names, or the vocabulary's primary ones (Wave 4a
/// §7.4). A kind the pack marks sensitive is refused by name and left out
/// by default; a name the registry does not hold is refused, before
/// anything is planned, whatever the layout.
fn observation_kinds(
    store: &mut Store,
    settings: &Settings,
) -> Result<Vec<nils_registry::clinical::Kind>, Error> {
    use nils_registry::clinical;
    if settings.observations.is_empty() {
        return Ok(clinical::observation_types(store)?
            .into_iter()
            .filter(|k| k.primary && !k.sensitive)
            .collect());
    }
    let mut found = Vec::new();
    for name in settings.observations {
        match clinical::kind_named(store, name)? {
            Some(k) if k.sensitive => {
                return Err(Error::Refused(format!(
                    "--observation {name}: the pack marks that kind sensitive, and a release never writes one (§7.4)"
                )));
            }
            Some(k) => found.push(k),
            None => {
                return Err(Error::Refused(format!(
                    "--observation {name} names no observation kind the registry holds"
                )));
            }
        }
    }
    Ok(found)
}

/// An observation kind's name as a column: lower case, one underscore for
/// each run of anything that is not a letter or a digit.
fn column_of(name: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            gap = false;
        } else if !gap && !out.is_empty() {
            out.push('_');
            gap = true;
        }
    }
    out.trim_end_matches('_').to_string()
}

fn write_dataset(
    settings: &Settings,
    report: &Report,
    scans: &BTreeMap<(String, String), Vec<crate::bids::dataset::Scan>>,
    codes: &[String],
    clinical: &Clinical,
) -> Result<(), std::io::Error> {
    use crate::bids::dataset;
    let root = settings.root;
    let made_by = dataset::MadeBy {
        name: "nils".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        dataset_version: report.version.clone(),
        converter: report.converter.clone(),
        policy: report.policy.clone(),
        pack: format!("{}@{}", settings.pack.name, settings.pack.version),
        placements: report.placements.clone(),
        authors: match settings.authors.is_empty() {
            true => vec![settings.actor.to_string()],
            false => settings.authors.to_vec(),
        },
    };
    std::fs::create_dir_all(root)?;
    std::fs::write(
        root.join("dataset_description.json"),
        dataset::description(settings.name, &made_by, None),
    )?;
    std::fs::write(
        root.join("README"),
        dataset::readme(settings.name, &made_by, &report.routes),
    )?;

    // One row per subject the release wrote, and nothing about them that the
    // policy did not let out.
    let rows: Vec<dataset::Participant> = codes
        .iter()
        .map(|id| dataset::Participant {
            id: id.clone(),
            extra: clinical.participants.get(id).cloned().unwrap_or_default(),
        })
        .collect();
    std::fs::write(root.join("participants.tsv"), dataset::participants(&rows))?;

    // §9.4. The directory is named by the session scheme and the time is in
    // the standard's own column, so anything joining on a date reads the
    // column rather than parsing a directory name.
    let mut by_subject: BTreeMap<&String, Vec<dataset::Session>> = BTreeMap::new();
    for ((code, label), rows) in scans {
        let earliest = rows.iter().filter_map(|s| s.acq_time.clone()).min();
        by_subject.entry(code).or_default().push(dataset::Session {
            label: label.clone(),
            acq_time: earliest,
            extra: clinical
                .sessions
                .get(&(code.clone(), label.clone()))
                .cloned()
                .unwrap_or_default(),
        });
        let dir = root.join(format!("sub-{code}/ses-{label}"));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join(format!("sub-{code}_ses-{label}_scans.tsv")),
            dataset::scans(rows),
        )?;
    }
    for (code, mut sessions) in by_subject {
        sessions.sort_by(|a, b| a.label.cmp(&b.label));
        let dir = root.join(format!("sub-{code}"));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join(format!("sub-{code}_sessions.tsv")),
            dataset::sessions(&sessions),
        )?;
    }

    // §9.3. A `.bidsignore` that lists what is not there tells a reader the
    // tree holds things it does not, so the lines are the ones this release's
    // own choices need.
    let mut ignore: Vec<String> = Vec::new();
    if report.routes.contains_key("beside") {
        ignore.push("*/*/localizer/".to_string());
    }
    if report.routes.contains_key("unofficial") {
        ignore.push("*_localizer.*".to_string());
    }
    if !ignore.is_empty() {
        std::fs::write(root.join(".bidsignore"), dataset::bidsignore(&ignore))?;
    }

    // And `derivatives/nils/` is a dataset in its own right, so the tree stays
    // valid and the data stays present.
    if report.routes.contains_key("derivatives") {
        let dir = root.join("derivatives/nils");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join("dataset_description.json"),
            dataset::derivative_description(settings.name, &made_by),
        )?;
    }
    Ok(())
}

/// One review item per study whose functional stacks nobody has placed (§9.2).
///
/// **Per study, not per stack.** A study is one protocol run on one occasion,
/// so what the subject was doing is a property of it, and a person who knows
/// the answer for one of its functional stacks knows it for all of them. v0
/// asks about 84 percent of its stacks; this asks once per occasion, and the
/// answer is a decision on the `task` axis scoped to the study.
fn ask_about_tasks(
    store: &mut Store,
    report: &Report,
    absent: &[(i64, String, String)],
    settings: &Settings,
) -> Result<(), Error> {
    let mut stacks: Vec<i64> = absent
        .iter()
        .filter(|(_, kind, _)| kind == "no_task")
        .map(|(stack, _, _)| *stack)
        .collect();
    if stacks.is_empty() {
        return Ok(());
    }
    stacks.sort_unstable();
    // The studies those stacks belong to, which is what is asked about.
    let sql = format!(
        "SELECT DISTINCT se.study_id FROM {} k JOIN {} se ON se.id = k.series_id \
         WHERE k.id IN ({})",
        store.qualified("stack"),
        store.qualified("series"),
        stacks
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut studies: Vec<i64> = Vec::new();
    for r in store.query(&sql, &[])? {
        studies.push(r.int(0)?);
    }
    studies.sort_unstable();
    let choices: Vec<&str> = settings.pack.bids.task.keys().map(String::as_str).collect();
    let now = now_iso();
    let rows: Vec<Vec<Param>> = studies
        .iter()
        .map(|study| {
            vec![
                Param::from("release.no_task"),
                Param::from("study"),
                Param::from(serde_json::json!({ "study_id": study }).to_string()),
                Param::from(
                    serde_json::json!({
                        "release": report.release_id,
                        "axis": "task",
                        "choices": choices,
                        "why": "func requires task and nothing in a DICOM says what a \
                                subject was doing",
                    })
                    .to_string(),
                ),
                Param::from("open"),
                Param::from(now.as_str()),
            ]
        })
        .collect();
    write_rows(
        store,
        "review_item",
        &["kind", "scope", "ref", "evidence", "status", "created_at"],
        &rows,
    )
}

/// Where a stack's files go, given the route it took (§9.3).
///
/// Both layouts end here, which is what lets §8.6 compare a place without
/// knowing which layout it is in.
fn place_of(
    route: &crate::bids::place::Route,
    settings: &Settings,
    code: &str,
    label: &str,
    folder: &str,
    stem: &str,
    placed: Option<&Placed>,
) -> Place {
    use crate::bids::place::Route;
    let session = format!("sub-{code}/ses-{label}");
    if settings.layout == Layout::Descriptive {
        return Place::dir(format!("{session}/{folder}/{stem}"));
    }
    match route {
        // The standard's own name, in the standard's own directory.
        Route::Raw => match placed.and_then(|p| p.bids.as_ref().ok()) {
            Some(n) => Place::file(n.dir(code, label), n.stem(code, label)),
            None => Place::dir(format!("{session}/{folder}/{stem}")),
        },
        // Kept as DICOM, one directory per stack, which is what a reader of
        // them wants anyway. Named by §9.1, which is most of why §9.1 names
        // everything.
        Route::SourceData => Place::dir(format!("sourcedata/{session}/{folder}/{stem}")),
        // A dataset in its own right, so the tree stays valid and the data
        // stays present.
        Route::Derivatives => Place::file(
            format!("derivatives/nils/{session}/{folder}"),
            unofficial(code, label, stem),
        ),
        // Outside the standard, so the entity set is ours and a `.bidsignore`
        // line says the standard does not know it.
        Route::Beside(what) => Place::file(
            format!("{session}/{what}"),
            format!("{}_{what}", unofficial(code, label, stem)),
        ),
        Route::Unofficial(datatype) => Place::file(
            format!("{session}/{datatype}"),
            format!("{}_localizer", unofficial(code, label, stem)),
        ),
        // Never reached: a stack routed nowhere is out of the version before
        // a place is asked for.
        Route::Nowhere(_) => Place::dir(format!("{session}/{folder}/{stem}")),
    }
}

/// A BIDS-shaped name for something BIDS has no name for.
///
/// The descriptive name of §9.1 carried in `acq-`, which is where a label that
/// describes an acquisition belongs, reduced to what a BIDS label may spell.
/// Uniqueness comes from §9.1's own disambiguation, which already ran over the
/// session as the registry holds it.
fn unofficial(code: &str, label: &str, stem: &str) -> String {
    let acq: String = stem
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '+')
        .collect();
    match (label.is_empty(), acq.is_empty()) {
        (_, true) => format!("sub-{code}_ses-{label}"),
        (true, _) => format!("sub-{code}_acq-{acq}"),
        (false, _) => format!("sub-{code}_ses-{label}_acq-{acq}"),
    }
}

/// When each stack was acquired, as the archive holds it: the study's day and
/// the series' time.
///
/// Read from the registry rather than from the file, because the file is
/// scrubbed by the time anything asks and because the study's day is the
/// repaired one (§4).
fn acquisition_times(
    store: &mut Store,
    days: &HashMap<i64, Day>,
) -> Result<HashMap<i64, (Day, Option<String>)>, Error> {
    let t = table("series");
    let d = store.dialect();
    let time = d.text_of_qualified(
        Some("se"),
        t.column("series_time")
            .expect("series.series_time is a column"),
    );
    let sql = format!(
        "SELECT k.id, se.study_id, {time} FROM {} k JOIN {} se ON se.id = k.series_id",
        store.qualified("stack"),
        store.qualified("series"),
    );
    let mut out = HashMap::new();
    for r in store.query(&sql, &[])? {
        if let Some(day) = days.get(&r.int(1)?) {
            out.insert(r.int(0)?, (*day, r.opt_text(2)?.map(str::to_string)));
        }
    }
    Ok(out)
}

/// One acquisition time, under the release's date policy (§9.4 and §8.3).
///
/// The whole point of §9.4: the directory is named by the session scheme and
/// the time is carried in the standard's own slot, **under the same policy the
/// files are under**. A release that shifted its dates writes the shifted time
/// here, and one that kept only the year writes nothing, because a time whose
/// date was truncated is not a time.
fn under_policy(
    (day, time): &(Day, Option<String>),
    settings: &Settings,
    offset: crate::dates::Offset,
) -> Option<String> {
    let day = match settings.policy.dates {
        crate::dates::Policy::Keep => *day,
        crate::dates::Policy::Shift => Day::from_days(day.to_days() + offset.0),
        crate::dates::Policy::Year => return None,
    };
    let stamp = format!("{:04}-{:02}-{:02}", day.year(), day.month(), day.day());
    let Some(t) = time.as_deref().map(str::trim).filter(|t| t.len() >= 6) else {
        return Some(stamp);
    };
    // `HHMMSS` or `HH:MM:SS`, either of which the store may hand back.
    let digits: String = t.chars().filter(char::is_ascii_digit).collect();
    match digits.len() >= 6 {
        true => Some(format!(
            "{stamp}T{}:{}:{}",
            &digits[0..2],
            &digits[2..4],
            &digits[4..6]
        )),
        false => Some(stamp),
    }
}

/// The root, and the room (§9.6).
///
/// The estimate is the source's own size, which is the right order for a
/// descriptive release (the same pixels, a smaller header) and generous for a
/// BIDS one (compressed NIfTI is smaller than the DICOM it came from). Being
/// generous is the point: a release refused for want of room somebody has is a
/// nuisance, and one that fills a filesystem at three in the morning is an
/// incident.
fn preflight(root: &Path, wanted: i64) -> Result<(), Error> {
    std::fs::create_dir_all(root)
        .map_err(|e| Error::Refused(format!("{} cannot be written into ({e})", root.display())))?;
    // A file, because a directory that cannot be written into is a permission
    // problem and reads as one, rather than as a full disk later.
    let probe = root.join(".nils-writable");
    std::fs::write(&probe, b"nils")
        .map_err(|e| Error::Refused(format!("{} is not writable ({e})", root.display())))?;
    std::fs::remove_file(&probe).ok();

    let Some(free) = free_bytes(root) else {
        return Ok(());
    };
    if free < wanted {
        return Err(Error::Refused(format!(
            "{} has {:.1} GiB free and this release is about {:.1} GiB of source \
             (§9.6). Free some room, or release a smaller selection.",
            root.display(),
            free as f64 / (1u64 << 30) as f64,
            wanted as f64 / (1u64 << 30) as f64,
        )));
    }
    Ok(())
}

/// What the filesystem says it has, or nothing where it will not say.
#[cfg(unix)]
#[allow(
    unsafe_code,
    reason = "statvfs fills a plain struct through the pointer it is given"
)]
fn free_bytes(path: &Path) -> Option<i64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: `c` is a valid NUL-terminated path and the pointer is to a
    // struct of the right type that lives for the whole call.
    let rc = unsafe { libc::statvfs(c.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: statvfs returned 0, so the struct is initialised.
    let stat = unsafe { stat.assume_init() };
    i64::try_from(stat.f_bavail)
        .ok()?
        .checked_mul(i64::try_from(stat.f_frsize).ok()?)
}

#[cfg(not(unix))]
fn free_bytes(_path: &Path) -> Option<i64> {
    // Windows has an answer and no crate we already depend on asks for it. A
    // release there gets the operating system's error at the moment it runs
    // out, which is what every release got before this.
    None
}

/// The day this release is made, for its version.
fn today() -> Day {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| Day::from_unix(d.as_secs() as i64))
        .unwrap_or_else(|| Day::new(1970, 1, 1).expect("the epoch is a day"))
}

/// The policy, as it applies to one subject.
///
/// The pseudonym and the date offset are in it because both are drawn from the
/// key, and neither is anywhere else in the content digest. A release re-run
/// under a different key writes different bytes into a differently named tree,
/// and a comparison that could not see that would call it unchanged.
fn subject_policy(settings: &Settings, code: &str, offset: crate::dates::Offset) -> String {
    format!(
        "{} code={code} offset={} unknown={} layout={} converter={}",
        settings.policy.as_json(),
        offset.0,
        settings.on_unknown.name(),
        settings.layout.name(),
        // A different converter writes different NIfTI, so an upgrade rewrites
        // the tree, which is what an upgrade means. It is also what lets a
        // conversion that failed be planned as failed again: nothing about the
        // stack or the converter changed, so nothing about the answer will.
        settings.converter.map(|c| c.describe()).unwrap_or_default(),
    )
}

/// The dataset a name and a root are a version of, made on first sight.
fn dataset_of(store: &mut Store, name: &str, root: &Path) -> Result<i64, Error> {
    let d = store.dialect();
    let sql = format!(
        "SELECT id FROM {} WHERE name = {} AND root = {}",
        store.qualified("dataset"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
    );
    let params = [Param::from(name), Param::from(root.display().to_string())];
    if let Some(r) = store.query_opt(&sql, &params)? {
        return Ok(r.int(0)?);
    }
    let written = store.insert(
        &Insert::new(table("dataset"), &["name", "root", "created_at"]).returning(&["id"]),
        &[vec![
            Param::from(name),
            Param::from(root.display().to_string()),
            Param::from(now_iso()),
        ]],
    )?;
    Ok(written.first().map(|r| r.int(0)).transpose()?.unwrap_or(0))
}

/// The newest finished version of a dataset, by id and not by the version
/// string, because `2026.09.05.10` sorts before `2026.09.05.9` and the tenth
/// version of a day is not the second.
fn newest_version(store: &mut Store, dataset: i64) -> Result<Option<Earlier>, Error> {
    let d = store.dialect();
    let sql = format!(
        "SELECT id, version FROM {} WHERE dataset_id = {} AND finished_at IS NOT NULL \
         ORDER BY id DESC LIMIT 1",
        store.qualified("release"),
        d.param(1, Type::Int),
    );
    let Some(r) = store.query_opt(&sql, &[Param::Int(dataset)])? else {
        return Ok(None);
    };
    Ok(Some(Earlier {
        id: r.int(0)?,
        version: r.text(1)?.to_string(),
    }))
}

/// Rename every moved stack's files, and say which arrived.
///
/// In two phases, through a staging directory, because two stacks can swap
/// names between versions: a disambiguating suffix moves when a sibling
/// appears or leaves, and renaming one onto the other in place would lose a
/// tree. A DICOM stack owns its directory and moves as one; a converted stack
/// is its named files and moves file by file.
fn move_them(root: &Path, jobs: &[Job]) -> std::collections::HashSet<i64> {
    let mut arrived = std::collections::HashSet::new();
    let moving: Vec<&Job> = jobs
        .iter()
        .filter(|j| j.change == crate::version::Change::Moved && j.was.is_some())
        .collect();
    if moving.is_empty() {
        return arrived;
    }
    let staging = root.join(".nils-moving");
    if std::fs::create_dir_all(&staging).is_err() {
        return arrived;
    }
    let mut staged: Vec<(&Job, Vec<Leg>)> = Vec::new();
    for job in &moving {
        let Some(was) = &job.was else { continue };
        let pairs: Vec<(PathBuf, PathBuf)> = match (was.files(), &job.place.stem) {
            (Files::Directory(dir), _) => vec![(root.join(dir), root.join(&job.place.dir))],
            (Files::Named(paths), Some(stem)) => {
                let old = format!(
                    "{}/{}",
                    was.place.dir,
                    was.place.stem.clone().unwrap_or_default()
                );
                paths
                    .iter()
                    .map(|p| {
                        let ext = p.strip_prefix(&old).unwrap_or("");
                        (
                            root.join(p),
                            root.join(format!("{}/{stem}{ext}", job.place.dir)),
                        )
                    })
                    .collect()
            }
            (Files::Named(_), None) => Vec::new(),
        };
        let mut mine = Vec::new();
        let mut whole = !pairs.is_empty();
        for (n, (from, to)) in pairs.into_iter().enumerate() {
            let park = staging.join(format!("{}-{n}", job.stack));
            match std::fs::rename(&from, &park) {
                Ok(()) => mine.push((from, park, to)),
                Err(_) => whole = false,
            }
        }
        // Half a move is not a move: what did reach the staging area is put
        // back, and the stack is written from scratch, because a tree holding
        // some of a stack under each of two names is worse than one holding
        // it under neither.
        match whole {
            true => staged.push((job, mine)),
            false => {
                for (from, park, _) in mine {
                    std::fs::rename(&park, &from).ok();
                }
            }
        }
    }
    for (job, mine) in staged {
        let mut whole = true;
        for (_, park, to) in &mine {
            if let Some(parent) = to.parent()
                && std::fs::create_dir_all(parent).is_err()
            {
                whole = false;
                continue;
            }
            if std::fs::rename(park, to).is_err() {
                whole = false;
            }
        }
        if whole {
            arrived.insert(job.stack);
        }
        for (from, _, _) in &mine {
            prune(root, from.parent());
        }
    }
    std::fs::remove_dir_all(&staging).ok();
    arrived
}

/// One file's journey through a move: where it was, where it waits, where
/// it goes.
type Leg = (PathBuf, PathBuf, PathBuf);

/// Remove a stack's files, and any directory they leave empty.
fn drop_files(root: &Path, files: &Files) {
    match files {
        Files::Directory(dir) => {
            let full = root.join(dir);
            std::fs::remove_dir_all(&full).ok();
            prune(root, full.parent());
        }
        Files::Named(paths) => {
            for p in paths {
                let full = root.join(p);
                std::fs::remove_file(&full).ok();
                prune(root, full.parent());
            }
        }
    }
}

/// Walk up from an emptied directory, removing what is now empty.
///
/// Bounded by the release root, which it never removes: a version that dropped
/// a subject should not leave `sub-x/ses-1/anat` behind, and must not walk
/// above the tree it owns. `remove_dir` refusing a directory that still holds
/// something is the stopping condition.
fn prune(root: &Path, from: Option<&Path>) {
    let mut here = from.map(Path::to_path_buf);
    while let Some(path) = here {
        if path == root || !path.starts_with(root) {
            break;
        }
        if std::fs::remove_dir(&path).is_err() {
            break;
        }
        here = path.parent().map(Path::to_path_buf);
    }
}

/// The `WHERE` of a selection over the joined tables `i`, `k`, `se`, `su`,
/// `sf`, `so`, shared by the three readers below so they cannot disagree.
fn selection_where(store: &mut Store, selection: &Selection) -> String {
    let mut wheres: Vec<String> = Vec::new();
    if !selection.subjects.is_empty() {
        wheres.push(format!(
            "su.code IN ({})",
            selection
                .subjects
                .iter()
                .map(|s| format!("'{}'", s.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !selection.stacks.is_empty() {
        wheres.push(format!(
            "k.id IN ({})",
            selection
                .stacks
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(m) = &selection.modality {
        wheres.push(format!("se.modality = '{}'", m.replace('\'', "''")));
    }
    let axis = store.qualified("classification_axis");
    // Wave 4a §8: stacks by what the pack says they are. One value per row
    // (§6.1), so equality is equality; the values of one axis are
    // alternatives, and every axis named has to hold.
    let mut by_axis: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (name, value) in &selection.axes {
        by_axis
            .entry(name.as_str())
            .or_default()
            .push(value.as_str());
    }
    for (name, values) in by_axis {
        wheres.push(format!(
            "EXISTS (SELECT 1 FROM {axis} a WHERE a.stack_id = k.id AND a.axis = '{}' AND a.value IN ({}))",
            name.replace('\'', "''"),
            values
                .iter()
                .map(|v| format!("'{}'", v.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    // A stack the pack ruled out is not written, and saying so as a default
    // rather than as a flag is what keeps a release from carrying screenshots.
    if selection.dispositions.is_empty() {
        wheres.push(format!(
            "NOT EXISTS (SELECT 1 FROM {axis} a WHERE a.stack_id = k.id AND a.axis = 'disposition' \
             AND a.value = 'excluded')"
        ));
    } else {
        wheres.push(format!(
            "EXISTS (SELECT 1 FROM {axis} a WHERE a.stack_id = k.id AND a.axis = 'disposition' \
             AND a.value IN ({}))",
            selection
                .dispositions
                .iter()
                .map(|d| format!("'{}'", d.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !selection.roles.is_empty() {
        // One row per value (Wave 4a §6.1): a role is matched by equality.
        let any = selection
            .roles
            .iter()
            .map(|r| format!("'{}'", r.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(", ");
        wheres.push(format!(
            "EXISTS (SELECT 1 FROM {axis} a WHERE a.stack_id = k.id AND a.axis = 'role' \
             AND a.value IN ({any}))"
        ));
    }
    if selection.picked_only {
        wheres.push(format!(
            "EXISTS (SELECT 1 FROM {} ps JOIN {} p ON p.id = ps.pick_id \
             WHERE ps.stack_id = k.id AND p.withdrawn_at IS NULL)",
            store.qualified("pick_stack"),
            store.qualified("pick"),
        ));
    }
    match wheres.is_empty() {
        true => String::new(),
        false => format!(" AND {}", wheres.join(" AND ")),
    }
}

/// The joined tables every selection reader walks.
fn selection_from(store: &mut Store) -> String {
    format!(
        "FROM {instance} i \
         JOIN {stack} k ON k.id = i.stack_id \
         JOIN {series} se ON se.id = i.series_id \
         JOIN {subject} su ON su.id = se.subject_id \
         JOIN {source_file} sf ON sf.instance_id = i.id \
         JOIN {source} so ON so.id = sf.source_id",
        instance = store.qualified("instance"),
        stack = store.qualified("stack"),
        series = store.qualified("series"),
        subject = store.qualified("subject"),
        source_file = store.qualified("source_file"),
        source = store.qualified("source"),
    )
}

/// The subjects the selection reaches, in a fixed order.
fn selected_subjects(
    store: &mut Store,
    selection: &Selection,
) -> Result<Vec<(i64, String)>, Error> {
    let from = selection_from(store);
    let filter = selection_where(store, selection);
    let sql = format!(
        "SELECT DISTINCT se.subject_id, su.code {from} WHERE 1 = 1{filter} ORDER BY se.subject_id"
    );
    let mut out = Vec::new();
    for r in store.query(&sql, &[])? {
        out.push((r.int(0)?, r.text(1)?.to_string()));
    }
    Ok(out)
}

/// How much source the selection is, for the preflight.
fn selection_bytes(store: &mut Store, selection: &Selection) -> Result<i64, Error> {
    let from = selection_from(store);
    let filter = selection_where(store, selection);
    // Cast, because a sum of bigints is a numeric on Postgres and the store
    // reads a numeric only as text.
    let sql =
        format!("SELECT CAST(COALESCE(SUM(sf.size), 0) AS BIGINT) {from} WHERE 1 = 1{filter}");
    Ok(store
        .query_opt(&sql, &[])?
        .map(|r| r.int(0))
        .transpose()?
        .unwrap_or(0))
}

/// What a selection reaches, counted (Wave 4a §8, `nils select`): the
/// subjects, studies, stacks and files, and the bytes. Sessions are not
/// here, because a session is derived under a scheme on read and matched
/// then; the preview says which were asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preview {
    pub subjects: i64,
    pub studies: i64,
    pub stacks: i64,
    pub files: i64,
    pub bytes: i64,
}

pub fn preview(store: &mut Store, selection: &Selection) -> Result<Preview, Error> {
    let from = selection_from(store);
    let filter = selection_where(store, selection);
    let sql = format!(
        "SELECT COUNT(DISTINCT su.id), COUNT(DISTINCT se.study_id), COUNT(DISTINCT k.id),                 COUNT(*), CAST(COALESCE(SUM(sf.size), 0) AS BIGINT)          {from} WHERE 1 = 1{filter}"
    );
    let r = store.query(&sql, &[])?;
    let Some(r) = r.first() else {
        return Ok(Preview::default());
    };
    Ok(Preview {
        subjects: r.int(0)?,
        studies: r.int(1)?,
        stacks: r.int(2)?,
        files: r.int(3)?,
        bytes: r.int(4)?,
    })
}

/// One subject's instances, with what the planner needs.
///
/// One subject at a time, so that the plan's memory is a function of the
/// largest subject and not of the archive.
fn select_subject(
    store: &mut Store,
    selection: &Selection,
    subject: i64,
) -> Result<Vec<Instance>, Error> {
    let from = selection_from(store);
    let filter = selection_where(store, selection);
    let sql = format!(
        // The stored path is relative to the batch root it was walked from,
        // which is what makes a registry portable; the root is on `source`.
        "SELECT i.id, k.id, se.study_id, so.root, sf.path, \
                sf.size, sf.mtime_ns \
         {from} WHERE se.subject_id = {}{filter} \
         ORDER BY se.study_id, k.id, i.id",
        store.dialect().param(1, Type::Int),
    );
    rows_to_instances(store.query(&sql, &[Param::Int(subject)])?)
}

/// One stack's instances, read only when the stack is written.
fn instances_of(store: &mut Store, stack: i64) -> Result<Vec<Instance>, Error> {
    let from = selection_from(store);
    let sql = format!(
        "SELECT i.id, k.id, se.study_id, so.root, sf.path, \
                sf.size, sf.mtime_ns \
         {from} WHERE k.id = {} ORDER BY i.id",
        store.dialect().param(1, Type::Int),
    );
    rows_to_instances(store.query(&sql, &[Param::Int(stack)])?)
}

fn rows_to_instances(rows: Vec<nils_registry::store::Row>) -> Result<Vec<Instance>, Error> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for r in rows {
        let id = r.int(0)?;
        // An instance may sit under more than one path (Wave 1's duplicate
        // handling); one copy of it is written.
        if !seen.insert(id) {
            continue;
        }
        out.push(Instance {
            id,
            stack: r.int(1)?,
            study: r.int(2)?,
            path: Path::new(r.text(3)?).join(r.text(4)?).display().to_string(),
            size: r.int(5)?,
            mtime: r.int(6)?,
        });
    }
    Ok(out)
}

fn study_days(store: &mut Store) -> Result<HashMap<i64, Day>, Error> {
    // Both dates through the dialect's own rendering. Postgres hands a `date`
    // back in a type the store reads only as text, and the release read it raw
    // until a release was run on Postgres.
    let t = table("study");
    let d = store.dialect();
    let day = |c: &str| {
        d.text_of(
            t.column(c)
                .unwrap_or_else(|| panic!("study.{c} is not a column")),
        )
    };
    let (filled, study) = (day("date_filled"), day("study_date"));
    let sql = format!(
        "SELECT id, COALESCE({filled}, {study}) FROM {} \
         WHERE COALESCE({filled}, {study}) IS NOT NULL",
        store.qualified("study")
    );
    let mut out = HashMap::new();
    for r in store.query(&sql, &[])? {
        if let Some(day) = Day::parse(r.text(1)?) {
            out.insert(r.int(0)?, day);
        }
    }
    Ok(out)
}

/// The session label of each of a subject's studies.
fn session_labels(
    mine: &[Instance],
    days: &HashMap<i64, Day>,
    scheme: &Scheme,
) -> HashMap<i64, String> {
    let mut studies: Vec<session::Study> = Vec::new();
    let mut seen: Vec<i64> = Vec::new();
    for i in mine {
        if seen.contains(&i.study) {
            continue;
        }
        let Some(day) = days.get(&i.study) else {
            continue;
        };
        seen.push(i.study);
        studies.push(session::Study::new(i.study, *day));
    }
    let anchor = studies.iter().map(|s| s.day).min();
    let mut out = HashMap::new();
    for occasion in session::sessions(&studies, anchor, scheme) {
        // A session with no label is named by the day it opened, which is what
        // `keep_date` means and why §4.3 refuses the combination that would
        // put a date here under a policy that moves them.
        let label = occasion
            .label
            .clone()
            .unwrap_or_else(|| occasion.first.compact());
        for study in &occasion.studies {
            out.insert(*study, label.clone());
        }
    }
    out
}

struct Written {
    wrote: Wrote,
    applied: scrub::Applied,
}

/// Read one file whole, apply the plan, write it out.
///
/// Read whole, because a release writes the pixels. The digest reader stops
/// before them, which is right for reading a header at speed and wrong here.
fn write_one(instance: &Instance, plan: &Plan, root: &Path, dir: &str) -> Result<Written, String> {
    let mut object = open(Path::new(&instance.path))?;
    let applied = scrub::apply(&mut object, plan);

    let relative = PathBuf::from(dir).join(format!("{:08}.dcm", instance.id));
    let target = root.join(&relative);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|_| "no directory to write into".to_string())?;
    }
    // Written whole and then moved into place, so that a run interrupted
    // halfway leaves no half a file for a handover to checksum.
    let temporary = target.with_extension("dcm.part");
    object
        .write_to_file(&temporary)
        .map_err(|e| format!("unwritable: {}", first_line(&e.to_string())))?;
    let bytes = std::fs::read(&temporary).map_err(|_| "unreadable after writing".to_string())?;
    // BLAKE2b-256 of what was written, so a handover can verify a tree
    // without reading the files back through a DICOM parser (§11).
    let digest = hex::encode(digest_of(&bytes));
    std::fs::rename(&temporary, &target)
        .map_err(|_| "could not be moved into place".to_string())?;

    Ok(Written {
        wrote: Wrote {
            path: relative.display().to_string(),
            digest,
            bytes: bytes.len() as i64,
        },
        applied,
    })
}

/// One stack, as DICOM in a directory of its own: the descriptive layout and
/// `sourcedata/` both.
fn write_dicom(job: &Job, plan: &Plan, root: &Path, dir: &str) -> Vec<Result<Written, String>> {
    job.instances
        .iter()
        .map(|i| write_one(i, plan, root, dir))
        .collect()
}

/// One stack in the BIDS layout (§9.2 to §9.6).
///
/// The files are scrubbed into a staging directory first and converted from
/// there, **never from the source**. `dcm2niix` writes its sidecar from the
/// DICOM headers and its own anonymiser is not a de-identification: measured on
/// `v1.0.20260724`, `-ba y` leaves `InstitutionName` and `AcquisitionTime` in
/// the JSON. Converting the scrubbed file means the sidecar inherits the
/// release's policy for free, including its dates.
fn write_bids(
    job: &mut Job,
    plan: &Plan,
    settings: &Settings,
    report: &mut Report,
) -> Vec<Result<Written, String>> {
    let root = settings.root;
    let staging = root.join(".nils-convert").join(job.stack.to_string());
    std::fs::remove_dir_all(&staging).ok();
    if std::fs::create_dir_all(&staging).is_err() {
        return vec![Err("no directory to stage into".to_string())];
    }
    // `sourcedata` is the source, so the scrub is the whole of the work.
    if job.route == "sourcedata" {
        std::fs::remove_dir_all(&staging).ok();
        return write_dicom(job, plan, root, &job.place.dir);
    }

    let mut staged: Vec<PathBuf> = Vec::new();
    let mut applied = scrub::Applied::default();
    let mut refused: Vec<Result<Written, String>> = Vec::new();
    for i in &job.instances {
        match stage_one(i, plan, &staging) {
            Ok((path, done)) => {
                staged.push(path);
                for (what, n) in done.changes {
                    *applied.changes.entry(what).or_insert(0) += n;
                }
            }
            Err(why) => refused.push(Err(why)),
        }
    }
    let Some(converter) = settings.converter else {
        std::fs::remove_dir_all(&staging).ok();
        refused.push(Err("no converter, and a BIDS tree is NIfTI".to_string()));
        return refused;
    };
    let stem = job.place.stem.clone().unwrap_or_default();
    let into = root.join(&job.place.dir);
    let made = crate::bids::convert::convert(
        converter,
        &staged,
        &staging,
        &into,
        &stem,
        settings.compress,
    );
    match made {
        Ok(made) => {
            std::fs::remove_dir_all(&staging).ok();
            let mut out = refused;
            let mut first = true;
            for file in made.files {
                let path = format!("{}/{file}", job.place.dir);
                match std::fs::read(root.join(&path)) {
                    Ok(bytes) => out.push(Ok(Written {
                        wrote: Wrote {
                            path,
                            digest: hex::encode(digest_of(&bytes)),
                            bytes: bytes.len() as i64,
                        },
                        // The tags changed are the stack's and not each file's,
                        // so they are counted once rather than per output.
                        applied: match std::mem::take(&mut first) {
                            true => std::mem::take(&mut applied),
                            false => scrub::Applied::default(),
                        },
                    })),
                    Err(_) => out.push(Err("unreadable after converting".to_string())),
                }
            }
            out
        }
        Err(why) => {
            // The data stays present. A stack the converter will not convert
            // is written as DICOM under `sourcedata/`, with the reason
            // reported: v0 carries a hard-coded list of vendors instead, so a
            // stack it could have converted is skipped and one it cannot is a
            // failure.
            std::fs::remove_dir_all(&staging).ok();
            *report.unconvertible.entry(first_line(&why)).or_insert(0) += 1;
            *report.routes.entry("sourcedata".to_string()).or_insert(0) += 1;
            if let Some(n) = report.routes.get_mut(job.route.as_str()) {
                *n -= 1;
            }
            job.route = "sourcedata".to_string();
            job.place = job.fallback.clone();
            let mut out = refused;
            out.extend(write_dicom(job, plan, root, &job.place.dir));
            out
        }
    }
}

/// Scrub one instance into the staging directory, ready to convert.
fn stage_one(
    instance: &Instance,
    plan: &Plan,
    staging: &Path,
) -> Result<(PathBuf, scrub::Applied), String> {
    let mut object = open(Path::new(&instance.path))?;
    let applied = scrub::apply(&mut object, plan);
    let path = staging.join(format!("{:08}.dcm", instance.id));
    object
        .write_to_file(&path)
        .map_err(|e| format!("unwritable: {}", first_line(&e.to_string())))?;
    Ok((path, applied))
}

/// Open a file whole, part 10 or bare.
///
/// A release writes conformant part 10 whatever it read, which means a bare
/// data set gains the file meta group it never had, built from what the data
/// set itself says. 152 of the 3,028 files of the test archive are bare, and a
/// release that refused them would silently drop five percent of a dataset.
fn open(path: &Path) -> Result<dicom_object::DefaultDicomObject, String> {
    use dicom_object::file::ReadPreamble;
    use dicom_object::{FileMetaTableBuilder, InMemDicomObject, OpenFileOptions};
    use dicom_transfer_syntax_registry::TransferSyntaxIndex;
    if let Ok(object) = OpenFileOptions::new()
        .read_preamble(ReadPreamble::Auto)
        .open_file(path)
    {
        return Ok(object);
    }
    // No file meta group. The transfer syntax is not written down anywhere in
    // a bare data set, so it is inferred exactly as the reader of Wave 1
    // infers it, by whether the first element carries a VR.
    for name in explicit_first(path) {
        let Some(ts) = dicom_transfer_syntax_registry::TransferSyntaxRegistry.get(name) else {
            continue;
        };
        let Ok(file) = std::fs::File::open(path) else {
            return Err("unreadable".to_string());
        };
        let Ok(dataset) = InMemDicomObject::read_dataset_with_ts(std::io::BufReader::new(file), ts)
        else {
            continue;
        };
        // The class and the instance are what a meta table is built from, and
        // a data set with neither is not an object to release.
        let Ok(object) = dataset.with_meta(FileMetaTableBuilder::new().transfer_syntax(name))
        else {
            continue;
        };
        return Ok(object);
    }
    Err("unreadable".to_string())
}

/// The two transfer syntaxes a bare data set may be in, likeliest first.
fn explicit_first(path: &Path) -> [&'static str; 2] {
    use std::io::Read;
    let mut head = [0u8; 6];
    let explicit = std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && head[4].is_ascii_uppercase()
        && head[5].is_ascii_uppercase();
    if explicit {
        ["1.2.840.10008.1.2.1", "1.2.840.10008.1.2"]
    } else {
        ["1.2.840.10008.1.2", "1.2.840.10008.1.2.1"]
    }
}

/// The digest of what was written.
fn digest_of(bytes: &[u8]) -> [u8; 32] {
    use blake2::Digest;
    let mut h = blake2::Blake2s256::new();
    h.update(bytes);
    let out = h.finalize();
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&out);
    digest
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or(text).to_string()
}

/// The offset a subject's dates moved by, kept with the identifiers rather
/// than beside the images: it is the thing that undoes the policy.
fn remember_offset(
    registry: &mut Registry,
    subject: i64,
    offset: crate::dates::Offset,
) -> Result<(), Error> {
    let mut store = registry
        .open_linkage()
        .map_err(|e| Error::Refused(e.to_string()))?;
    let sql = format!(
        "SELECT offset_days FROM {} WHERE subject_id = {}",
        store.qualified("date_shift"),
        store.dialect().param(1, Type::Int)
    );
    if store.query_opt(&sql, &[Param::Int(subject)])?.is_some() {
        return Ok(());
    }
    store.insert(
        &Insert::new(table("date_shift"), &["subject_id", "offset_days"]),
        &[vec![Param::Int(subject), Param::Int(offset.0)]],
    )?;
    Ok(())
}

fn open_row(
    store: &mut Store,
    settings: &Settings,
    version: &str,
    dataset: i64,
    earlier: Option<&Earlier>,
    placements: &BTreeMap<String, String>,
) -> Result<i64, Error> {
    let categories: Vec<&str> = settings.categories.iter().map(|c| c.name()).collect();
    let written = store.insert(
        &Insert::new(
            table("release"),
            &[
                "name",
                "dataset_id",
                "version",
                "previous_id",
                "root",
                "policy",
                "selection",
                "categories",
                "session_scheme",
                "layout",
                "placements",
                "converter",
                "pack",
                "pack_version",
                "actor",
                "started_at",
                "files",
                "subjects",
                "unchanged",
                "moved",
                "rewritten",
                "added",
                "removed",
            ],
        )
        .returning(&["id"]),
        &[vec![
            Param::from(settings.name),
            Param::Int(dataset),
            Param::from(version),
            match earlier {
                Some(p) => Param::Int(p.id),
                None => Param::Null,
            },
            Param::from(settings.root.display().to_string()),
            Param::from(settings.policy.as_json().to_string()),
            Param::from(settings.selection.as_json().to_string()),
            Param::from(categories.join(",")),
            Param::from(
                serde_json::to_string(settings.scheme).unwrap_or_else(|_| "{}".to_string()),
            ),
            Param::from(settings.layout.name()),
            Param::from(serde_json::json!(placements).to_string()),
            match settings.converter {
                Some(c) => Param::from(c.describe()),
                None => Param::Null,
            },
            Param::from(settings.pack.name.as_str()),
            Param::from(settings.pack.version.to_string()),
            Param::from(settings.actor),
            Param::from(now_iso()),
            // Filled in when the run closes; a run that never closed says it
            // did nothing, which is what a tree it half wrote is worth.
            Param::Int(0),
            Param::Int(0),
            Param::Int(0),
            Param::Int(0),
            Param::Int(0),
            Param::Int(0),
            Param::Int(0),
        ]],
    )?;
    Ok(written.first().map(|r| r.int(0)).transpose()?.unwrap_or(0))
}

fn close_row(store: &mut Store, report: &Report) -> Result<(), Error> {
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET finished_at = {}, files = {}, subjects = {}, unchanged = {}, moved = {}, \
         rewritten = {}, added = {}, removed = {} WHERE id = {}",
        store.qualified("release"),
        d.param(1, Type::Timestamp),
        d.param(2, Type::Int),
        d.param(3, Type::Int),
        d.param(4, Type::Int),
        d.param(5, Type::Int),
        d.param(6, Type::Int),
        d.param(7, Type::Int),
        d.param(8, Type::Int),
        d.param(9, Type::Int),
    );
    store.execute(
        &sql,
        &[
            Param::from(now_iso()),
            Param::Int(report.files),
            Param::Int(report.subjects),
            Param::Int(report.unchanged),
            Param::Int(report.moved),
            Param::Int(report.rewritten),
            Param::Int(report.added),
            Param::Int(report.removed),
            Param::Int(report.release_id),
        ],
    )?;
    Ok(())
}

/// Insert a batch, or leave the table as it was.
fn write_rows(
    store: &mut Store,
    into: &str,
    columns: &[&str],
    rows: &[Vec<Param>],
) -> Result<(), Error> {
    if rows.is_empty() {
        return Ok(());
    }
    store.begin()?;
    match store.insert(&Insert::new(table(into), columns), rows) {
        Ok(_) => {
            store.commit()?;
            Ok(())
        }
        Err(e) => {
            store.rollback().ok();
            Err(Error::Store(e))
        }
    }
}

/// Where every stack in the registry goes, in both layouts (§9).
///
/// **Every** stack, and not only the selected ones. A name has to be unique in
/// the directory it lands in, and that directory holds what the registry holds
/// rather than what this release picked: v0 computes the same thing over the
/// already filtered list, so exporting one echo of a two-echo series drops the
/// echo suffix and the file is named as though it were the only one. The same
/// argument decides a BIDS `run-` index, which is the standard's answer to two
/// acquisitions that are otherwise the same thing.
fn places(
    store: &mut Store,
    days: &HashMap<i64, Day>,
    scheme: &Scheme,
    pack: &nils_pack::pack::Pack,
) -> Result<HashMap<i64, Placed>, Error> {
    let axes = axis_values(store)?;
    let t = table("stack_fingerprint");
    let d = store.dialect();
    let text = |c: &str| {
        d.text_of_qualified(
            Some("f"),
            t.column(c)
                .unwrap_or_else(|| panic!("stack_fingerprint.{c} is not a column")),
        )
    };
    // The text columns are cast so that a dialect's own rendering does not
    // decide the name; the numbers are read as numbers.
    let sql = format!(
        "SELECT f.stack_id, f.subject_id, f.study_id, f.series_id, f.stacks_in_series, \
                f.stack_index, {}, {}, {}, {}, {}, \
                f.inversion_time, f.dwi_b_value, f.dwi_directions \
         FROM {} f ORDER BY f.stack_id",
        text("orientation"),
        text("split_reason"),
        text("echo_numbers"),
        text("mr_acquisition_type"),
        text("dwi_pe_direction"),
        store.qualified("stack_fingerprint"),
    );

    // Bucket by subject, session and folder, which is the directory a name has
    // to be unique in.
    let mut buckets: BTreeMap<(i64, String, String), Vec<name::Named>> = BTreeMap::new();
    let mut labels: HashMap<i64, HashMap<i64, String>> = HashMap::new();
    let mut studies_of: BTreeMap<i64, Vec<(i64, Day)>> = BTreeMap::new();
    let rows = store.query(&sql, &[])?;
    for r in &rows {
        if let Some(day) = days.get(&r.int(2)?) {
            let mine = studies_of.entry(r.int(1)?).or_default();
            if !mine.iter().any(|(id, _)| *id == r.int(2).unwrap_or(0)) {
                mine.push((r.int(2)?, *day));
            }
        }
    }
    for (subject, studies) in &studies_of {
        let points: Vec<session::Study> = studies
            .iter()
            .map(|(id, day)| session::Study::new(*id, *day))
            .collect();
        let anchor = points.iter().map(|s| s.day).min();
        let mut mine = HashMap::new();
        for occasion in session::sessions(&points, anchor, scheme) {
            let label = occasion
                .label
                .clone()
                .unwrap_or_else(|| occasion.first.compact());
            for study in &occasion.studies {
                mine.insert(*study, label.clone());
            }
        }
        labels.insert(*subject, mine);
    }

    // The BIDS name of every stack, bucketed by the directory it has to be
    // unique in, which for BIDS is the datatype and not a directory per stack.
    let mut bids: HashMap<i64, Result<crate::bids::name::Name, crate::bids::name::Why>> =
        HashMap::new();
    // Ordered by series, then by the stack's index in it, then by its id, so
    // that two runs of one version assign the same `run-` numbers.
    type Ordered = Vec<(i64, i64, i64, String)>;
    let mut bids_buckets: BTreeMap<(i64, String, &'static str), Ordered> = BTreeMap::new();
    let mut extra: HashMap<i64, (bool, Option<String>)> = HashMap::new();

    for r in &rows {
        let stack = r.int(0)?;
        let empty = BTreeMap::new();
        let a = axes.get(&stack).unwrap_or(&empty);
        let get = |k: &str| a.get(k).map(String::as_str).filter(|v| !v.is_empty());
        let folder = folder_of(get("directory_type"), get("provenance"));
        // §9.2. Every axis value is read as an **identity** and not as what a
        // row stores: `base` stores `T2*w` and its identity is `T2starw`,
        // `post_contrast` stores `1` and its identity is `given`. Both names
        // go through this, because a name built on a stored label is a name
        // built on v0's column types.
        let id_of = |axis: &str, stored: &str| -> Option<String> {
            pack.axes
                .iter()
                .find(|x| x.name == axis)
                .and_then(|x| x.id_of_stored(stored))
                .map(str::to_string)
        };
        let ids_of = |axis: &str, stored: Option<&str>| -> Vec<String> {
            stored
                .into_iter()
                .flat_map(|v| v.split(','))
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .filter_map(|v| id_of(axis, v))
                .collect()
        };
        let contrast = get("post_contrast").and_then(|v| id_of("post_contrast", v));
        let contrast = contrast.as_deref() == Some("given");
        let fields = name::Fields {
            body_part: get("body_part"),
            spinal_cord: get("body_part") == Some("spine"),
            orientation: r.opt_text(6)?,
            base: get("base"),
            acquisition_type: r.opt_text(9)?,
            modifier: get("modifier"),
            technique: get("technique"),
            acceleration: get("acceleration"),
            construct: get("construct"),
            post_contrast: contrast,
            datatype: get("directory_type"),
            dwi_b_value: r.double(12).ok(),
            dwi_pe_direction: r.opt_text(10)?,
            dwi_directions: r.opt_int(13)?,
        };
        let subject = r.int(1)?;
        let study = r.int(2)?;
        let label = labels
            .get(&subject)
            .and_then(|m| m.get(&study))
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());

        let constructs = ids_of("construct", get("construct"));
        let modifiers = ids_of("modifier", get("modifier"));
        let technique = get("technique").and_then(|v| id_of("technique", v));
        let base = get("base").and_then(|v| id_of("base", v));
        let body_part = get("body_part").and_then(|v| id_of("body_part", v));
        let provenance = get("provenance").and_then(|v| id_of("provenance", v));
        let facts = crate::bids::name::Facts {
            intent: get("directory_type"),
            constructs: constructs.iter().map(String::as_str).collect(),
            technique: technique.as_deref(),
            modifiers: modifiers.iter().map(String::as_str).collect(),
            base: base.as_deref(),
            body_part: body_part.as_deref(),
            provenance: provenance.as_deref(),
            orientation: r.opt_text(6)?,
            acquisition_type: r.opt_text(9)?,
            post_contrast: contrast,
            // The one fact no rule sets: a person answers it per study (§9.2).
            task: get("task"),
            // The echo number, and only where the series has more than one
            // stack. §9.1's rule, for §9.1's reason: an `echo-1` on a series
            // with one echo says nothing and is in every filename.
            echo: match r.int(4)? > 1 {
                true => r.opt_text(8)?.and_then(first_int),
                false => None,
            },
            pe_direction: r.opt_text(10)?,
        };
        let built = crate::bids::name::build(&facts, &pack.bids);
        let synthetic = pack.bids.is_synthetic(
            provenance.as_deref(),
            &constructs.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        extra.insert(stack, (synthetic, get("disposition").map(str::to_string)));
        if let Ok(n) = &built {
            bids_buckets
                .entry((subject, label.clone(), n.datatype))
                .or_default()
                .push((r.int(3)?, r.int(5)?, stack, n.stem("s", "s")));
        }
        bids.insert(stack, built);

        buckets
            .entry((subject, label, folder.clone()))
            .or_default()
            .push(name::Named {
                stack,
                name: name::describe(&fields, true, true),
                folder,
                echo: r.opt_text(8)?.and_then(first_int),
                inversion_time: r.double(11).ok(),
                series: r.int(3)?,
                siblings: r.int(4)?,
                split: r.opt_text(7)?.map(str::to_string),
                index: r.int(5)?,
            });
    }

    // Two acquisitions that are otherwise the same thing are what `run-` is
    // for, and the standard has no other answer. In a fixed order, so that two
    // runs of one version agree: by series, then by the stack's index in it,
    // then by its id.
    for bucket in bids_buckets.values_mut() {
        bucket.sort();
        let mut counts: BTreeMap<&String, i64> = BTreeMap::new();
        for (_, _, _, stem) in bucket.iter() {
            *counts.entry(stem).or_insert(0) += 1;
        }
        let mut seen: BTreeMap<String, i64> = BTreeMap::new();
        for (_, _, stack, stem) in bucket.iter() {
            if counts.get(stem).copied().unwrap_or(0) < 2 {
                continue;
            }
            let n = seen.entry(stem.clone()).or_insert(0);
            *n += 1;
            // A group the standard gives no `run` to is one where two stacks
            // cannot be told apart in a BIDS name either. Left as it is, which
            // makes the second a rewrite of the first and is caught by the
            // collision check rather than hidden by a counter.
            if let Some(name) = bids.get(stack).and_then(|b| b.as_ref().ok())
                && let Some(with) = name.with_run(*n)
            {
                bids.insert(*stack, Ok(with));
            }
        }
    }

    let mut out = HashMap::new();
    for bucket in buckets.values_mut() {
        name::disambiguate(bucket);
        for n in bucket.iter() {
            let (synthetic, disposition) = extra.remove(&n.stack).unwrap_or((false, None));
            out.insert(
                n.stack,
                Placed {
                    descriptive: n.clone(),
                    bids: bids
                        .remove(&n.stack)
                        .unwrap_or(Err(crate::bids::name::Why::NoSuffix)),
                    synthetic,
                    disposition,
                },
            );
        }
    }
    Ok(out)
}

/// Where one stack goes, in both layouts (§9).
///
/// Both are computed for every stack whatever the release asked for, because
/// the BIDS layout falls back to the descriptive name wherever the standard has
/// no word: `sourcedata/` and `derivatives/nils/` are named by §9.1, which is
/// most of why §9.1 names everything.
struct Placed {
    descriptive: name::Named,
    bids: Result<crate::bids::name::Name, crate::bids::name::Why>,
    /// A vendor's synthetic contrast, which §9.3 lets a release place.
    synthetic: bool,
    disposition: Option<String>,
}

/// `EchoNumbers` may carry several values; the first is this stack's.
fn first_int(text: &str) -> Option<i64> {
    text.split(['\\', ','])
        .next()?
        .trim()
        .parse()
        .ok()
        .filter(|n: &i64| *n > 0)
}

/// Where a stack lands, which is its intent with v0's provenance routing.
///
/// v0 sends a SyMRI to `anat/SyMRI` under a flag, and an SWI, a STAGE and a
/// projection to `anat` whatever their intent says. The grouping is kept
/// because it is what people have on disk.
fn folder_of(datatype: Option<&str>, provenance: Option<&str>) -> String {
    match provenance {
        Some("SyMRI") => "anat/SyMRI".to_string(),
        Some("SWIRecon") | Some("STAGE") | Some("ProjectionDerived") => "anat".to_string(),
        _ => datatype.unwrap_or("misc").to_string(),
    }
}

/// Every decided axis of every stack.
fn axis_values(store: &mut Store) -> Result<HashMap<i64, BTreeMap<String, String>>, Error> {
    let sql = format!(
        "SELECT stack_id, axis, value FROM {}",
        store.qualified("classification_axis")
    );
    let mut out: HashMap<i64, BTreeMap<String, String>> = HashMap::new();
    for r in store.query(&sql, &[])? {
        if let Some(v) = r.opt_text(2)? {
            // One row per value; the naming grammar reads an axis as one
            // joined string and splits it, so the rows are joined back.
            out.entry(r.int(0)?)
                .or_default()
                .entry(r.text(1)?.to_string())
                .and_modify(|held| {
                    held.push(',');
                    held.push_str(v);
                })
                .or_insert_with(|| v.to_string());
        }
    }
    Ok(out)
}

/// What each stack's file says about its own pixels (§8.4).
///
/// Read from the stack and its series, not from the fingerprint, so that a
/// release after a digest alone judges as well as one after a fingerprint. A
/// check that quietly did nothing until some earlier verb had run is a check
/// nobody can rely on, and this is the one standing between a screenshot and a
/// dataset.
///
/// The fingerprint's `image_role` is joined in where it exists, because §6
/// worked the same three tokens out once and three things read it; where it
/// does not, the image type is read here and reaches the same answer.
fn pixel_verdicts(store: &mut Store) -> Result<HashMap<i64, crate::burned::Verdict>, Error> {
    let annotation = table("series")
        .column("burned_in_annotation")
        .expect("series.burned_in_annotation is a column");
    let image_type = table("stack")
        .column("image_type")
        .expect("stack.image_type is a column");
    let series_image_type = table("series")
        .column("image_type")
        .expect("series.image_type is a column");
    let d = store.dialect();
    let sql = format!(
        "SELECT k.id, {}, f.image_role, COALESCE({}, {}) \
         FROM {} k \
         JOIN {} se ON se.id = k.series_id \
         LEFT JOIN {} f ON f.stack_id = k.id",
        d.text_of_qualified(Some("se"), annotation),
        d.text_of_qualified(Some("k"), image_type),
        d.text_of_qualified(Some("se"), series_image_type),
        store.qualified("stack"),
        store.qualified("series"),
        store.qualified("stack_fingerprint"),
    );
    let mut out = HashMap::new();
    for r in store.query(&sql, &[])? {
        out.insert(
            r.int(0)?,
            crate::burned::judge(
                r.opt_text(1)?.map(str::trim),
                r.opt_text(2)?,
                r.opt_text(3)?,
            ),
        );
    }
    Ok(out)
}

/// What was changed, by tag and action (§8.5). No old value anywhere.
fn write_changes(store: &mut Store, report: &Report) -> Result<(), Error> {
    if report.changes.is_empty() {
        return Ok(());
    }
    let rows: Vec<Vec<Param>> = report
        .changes
        .iter()
        .map(|(what, n)| {
            let (tag, action) = what.rsplit_once(' ').unwrap_or((what.as_str(), ""));
            vec![
                Param::Int(report.release_id),
                Param::from(tag),
                Param::from(action),
                Param::Int(*n),
            ]
        })
        .collect();
    store.begin()?;
    let result = store.insert(
        &Insert::new(
            table("release_change"),
            &["release_id", "tag", "action", "count"],
        ),
        &rows,
    );
    match result {
        Ok(_) => {
            store.commit()?;
            Ok(())
        }
        Err(e) => {
            store.rollback().ok();
            Err(Error::Store(e))
        }
    }
}

/// One question per held stack, so a person can answer it and the release can
/// be run again.
fn raise_review(
    store: &mut Store,
    report: &Report,
    held: &std::collections::HashSet<i64>,
    pixels: &HashMap<i64, crate::burned::Verdict>,
) -> Result<(), Error> {
    let now = now_iso();
    let mut ids: Vec<i64> = held.iter().copied().collect();
    ids.sort_unstable();
    let rows: Vec<Vec<Param>> = ids
        .iter()
        .map(|stack| {
            let verdict = pixels
                .get(stack)
                .copied()
                .unwrap_or(crate::burned::Verdict::Unknown);
            vec![
                Param::from(format!("release.{}", verdict.name())),
                Param::from("stack"),
                Param::from(serde_json::json!({"stack_id": stack}).to_string()),
                Param::from(
                    serde_json::json!({
                        "release": report.release_id,
                        "verdict": verdict.name(),
                        "why": "the engine does not look at pixels; the file was asked",
                    })
                    .to_string(),
                ),
                Param::from("open"),
                Param::from(now.as_str()),
            ]
        })
        .collect();
    store.begin()?;
    let result = store.insert(
        &Insert::new(
            table("review_item"),
            &["kind", "scope", "ref", "evidence", "status", "created_at"],
        ),
        &rows,
    );
    match result {
        Ok(_) => {
            store.commit()?;
            Ok(())
        }
        Err(e) => {
            store.rollback().ok();
            Err(Error::Store(e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nils_dicom::synth::TempDir;

    fn state(place: Place, extensions: &[&str]) -> State {
        let files = match extensions.is_empty() {
            true => 1,
            false => extensions.len() as i64,
        };
        State {
            content: "same".to_string(),
            place,
            route: "raw".to_string(),
            files,
            bytes: files,
            digest: "d".to_string(),
            extensions: extensions.iter().map(|e| e.to_string()).collect(),
        }
    }

    fn job(stack: i64, was: &str, dir: &str) -> Job {
        Job {
            stack,
            place: Place::dir(dir.to_string()),
            fallback: Place::dir(dir.to_string()),
            route: "raw".to_string(),
            content: "same".to_string(),
            change: crate::version::Change::Moved,
            was: Some(state(Place::dir(was.to_string()), &[])),
            code: "x".to_string(),
            offset: crate::dates::Offset(0),
            instances: Vec::new(),
        }
    }

    fn write(root: &Path, path: &str) {
        let full = root.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, path.as_bytes()).unwrap();
    }

    fn read(root: &Path, path: &str) -> Option<String> {
        std::fs::read_to_string(root.join(path)).ok()
    }

    #[test]
    fn two_stacks_can_swap_names_between_versions() {
        // Which is why the moves go through a staging directory. A
        // disambiguating suffix moves when a sibling appears or leaves, and
        // renaming one onto the other in place would lose a tree.
        let dir = TempDir::new("move");
        let root = dir.path();
        write(root, "sub-x/ses-1/anat/T1w_1/00000001.dcm");
        write(root, "sub-x/ses-1/anat/T1w_2/00000002.dcm");
        let jobs = vec![
            job(1, "sub-x/ses-1/anat/T1w_1", "sub-x/ses-1/anat/T1w_2"),
            job(2, "sub-x/ses-1/anat/T1w_2", "sub-x/ses-1/anat/T1w_1"),
        ];
        let arrived = move_them(root, &jobs);
        assert_eq!(arrived.len(), 2);
        assert_eq!(
            read(root, "sub-x/ses-1/anat/T1w_2/00000001.dcm").as_deref(),
            Some("sub-x/ses-1/anat/T1w_1/00000001.dcm"),
            "the first went where the second was"
        );
        assert_eq!(
            read(root, "sub-x/ses-1/anat/T1w_1/00000002.dcm").as_deref(),
            Some("sub-x/ses-1/anat/T1w_2/00000002.dcm")
        );
        assert!(!root.join(".nils-moving").exists(), "and it tidied up");
    }

    #[test]
    fn a_bids_move_renames_the_files_and_not_a_directory() {
        // The two layouts differ in exactly this: in BIDS stacks share `anat/`
        // and are told apart by their names, so a move is a rename of files
        // inside one directory, and the state knows the files by their
        // extensions rather than by a manifest.
        let dir = TempDir::new("move-bids");
        let root = dir.path();
        write(root, "sub-x/ses-1/anat/sub-x_ses-1_T1w.nii.gz");
        write(root, "sub-x/ses-1/anat/sub-x_ses-1_T1w.json");
        let mut j = job(1, "sub-x/ses-1/anat", "sub-x/ses-1/anat");
        j.was = Some(state(
            Place::file("sub-x/ses-1/anat".into(), "sub-x_ses-1_T1w".into()),
            &[".nii.gz", ".json"],
        ));
        j.place = Place::file(
            "sub-x/ses-1/anat".to_string(),
            "sub-x_ses-1_acq-Spine_T1w".to_string(),
        );
        assert_eq!(move_them(root, &[j]).len(), 1);
        assert!(
            root.join("sub-x/ses-1/anat/sub-x_ses-1_acq-Spine_T1w.nii.gz")
                .exists()
        );
        assert!(
            root.join("sub-x/ses-1/anat/sub-x_ses-1_acq-Spine_T1w.json")
                .exists()
        );
        assert!(
            !root
                .join("sub-x/ses-1/anat/sub-x_ses-1_T1w.nii.gz")
                .exists()
        );
    }

    #[test]
    fn a_move_whose_source_is_gone_is_not_reported_as_a_move() {
        // Somebody emptied the tree. The stack is written again, which the
        // caller does by reading what did not arrive.
        let dir = TempDir::new("move-gone");
        let jobs = vec![job(1, "sub-x/ses-1/anat/T1w", "sub-x/ses-1/anat/SC_T1w")];
        assert!(move_them(dir.path(), &jobs).is_empty());
    }

    #[test]
    fn half_a_move_is_not_a_move() {
        // A tree holding some of a stack under each of two names is worse than
        // one holding it under neither, so the stack is written from scratch
        // and what did move is put back first.
        let dir = TempDir::new("move-half");
        let root = dir.path();
        write(root, "sub-x/ses-1/anat/sub-x_ses-1_T1w.nii.gz");
        let mut j = job(1, "sub-x/ses-1/anat", "sub-x/ses-1/anat");
        j.was = Some(state(
            Place::file("sub-x/ses-1/anat".into(), "sub-x_ses-1_T1w".into()),
            &[".nii.gz", ".json"],
        ));
        j.place = Place::file(
            "sub-x/ses-1/anat".into(),
            "sub-x_ses-1_acq-Spine_T1w".into(),
        );
        assert!(move_them(root, &[j]).is_empty());
        assert!(
            !root
                .join("sub-x/ses-1/anat/sub-x_ses-1_acq-Spine_T1w.nii.gz")
                .exists()
        );
        assert!(
            root.join("sub-x/ses-1/anat/sub-x_ses-1_T1w.nii.gz")
                .exists(),
            "what did move was put back"
        );
    }

    #[test]
    fn a_directory_that_empties_takes_its_parents_with_it() {
        // A version that dropped a subject should not leave `sub-x/ses-1/anat`
        // behind. The release root itself is never removed.
        let dir = TempDir::new("drop");
        let root = dir.path();
        write(root, "sub-x/ses-1/anat/T1w/00000001.dcm");
        write(root, "sub-y/ses-1/anat/T1w/00000002.dcm");
        drop_files(root, &Files::Directory("sub-x/ses-1/anat/T1w".into()));
        assert!(!root.join("sub-x").exists());
        assert!(root.join("sub-y/ses-1/anat/T1w").exists(), "and only that");
        assert!(root.exists());
    }

    #[test]
    fn a_parent_that_still_holds_something_stays() {
        let dir = TempDir::new("drop-sibling");
        let root = dir.path();
        write(root, "sub-x/ses-1/anat/T1w/00000001.dcm");
        write(root, "sub-x/ses-1/anat/T2w/00000002.dcm");
        drop_files(root, &Files::Directory("sub-x/ses-1/anat/T1w".into()));
        assert!(root.join("sub-x/ses-1/anat/T2w").exists());
        // And a converted stack's files, by name, leaving its siblings.
        write(root, "sub-x/ses-1/anat/sub-x_ses-1_T1w.nii.gz");
        write(root, "sub-x/ses-1/anat/sub-x_ses-1_T1w.json");
        write(root, "sub-x/ses-1/anat/sub-x_ses-1_T2w.nii.gz");
        let s = state(
            Place::file("sub-x/ses-1/anat".into(), "sub-x_ses-1_T1w".into()),
            &[".nii.gz", ".json"],
        );
        drop_files(root, &s.files());
        assert!(!root.join("sub-x/ses-1/anat/sub-x_ses-1_T1w.json").exists());
        assert!(
            root.join("sub-x/ses-1/anat/sub-x_ses-1_T2w.nii.gz")
                .exists()
        );
    }

    #[test]
    fn a_state_knows_its_files_without_a_manifest() {
        // The whole of Wave 4a §4's saving: a DICOM stack owns a directory,
        // a converted stack is its stem plus its extensions, and neither needs
        // a row per file to be found again.
        assert!(matches!(
            state(Place::dir("a/b".into()), &[]).files(),
            Files::Directory(d) if d == "a/b"
        ));
        assert!(matches!(
            state(Place::file("a/b".into(), "c".into()), &[".nii.gz", ".json"]).files(),
            Files::Named(v) if v == vec!["a/b/c.nii.gz".to_string(), "a/b/c.json".to_string()]
        ));
        assert_eq!(Place::dir("a/b".into()).key(), "a/b");
        assert_eq!(Place::file("a/b".into(), "c".into()).key(), "a/b/c");
    }

    #[test]
    fn a_bids_shaped_name_for_something_bids_has_no_name_for() {
        // The descriptive name carried in `acq-`, reduced to what a BIDS label
        // may spell, because a label is `[0-9a-zA-Z+]+` and ours are not.
        assert_eq!(
            unofficial("x", "M06", "Ax_T2w_2D_FLAIR-FatSat_SPACE"),
            "sub-x_ses-M06_acq-AxT2w2DFLAIRFatSatSPACE"
        );
        assert_eq!(unofficial("x", "", "T1w"), "sub-x_acq-T1w");
    }

    #[test]
    fn a_time_is_carried_under_the_policy_that_moved_it() {
        // §9.4 and §8.3: the tree's own column says what the files say.
        let day = Day::parse("20220115").unwrap();
        let scheme = session::Scheme::default();
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri");
        let pack = nils_pack::load(&dir, None).expect("the MRI pack loads");
        let mut settings = Settings {
            name: "t",
            root: Path::new("/tmp"),
            policy: &crate::policy::Policy::default(),
            categories: Vec::new(),
            selection: Selection::default(),
            scheme: &scheme,
            private: &[],
            on_unknown: crate::burned::OnUnknown::Write,
            actor: "t",
            key: b"k",
            pack: &pack,
            layout: Layout::Bids,
            places: crate::bids::place::Options::default(),
            converter: None,
            compress: true,
            observations: &[],
            authors: &[],
        };
        let when = (day, Some("031415".to_string()));
        assert_eq!(
            under_policy(&when, &settings, crate::dates::Offset(0)).as_deref(),
            Some("2022-01-15T03:14:15")
        );
        let shifted = crate::policy::Policy {
            dates: crate::dates::Policy::Shift,
            ..crate::policy::Policy::default()
        };
        settings.policy = &shifted;
        assert_eq!(
            under_policy(&when, &settings, crate::dates::Offset(-10)).as_deref(),
            Some("2022-01-05T03:14:15")
        );
        // A time whose date was truncated is not a time.
        let year = crate::policy::Policy {
            dates: crate::dates::Policy::Year,
            ..crate::policy::Policy::default()
        };
        settings.policy = &year;
        assert_eq!(
            under_policy(&when, &settings, crate::dates::Offset(0)),
            None
        );
    }
}
