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

use nils_registry::day::Day;
use nils_registry::schema::{Type, table};
use nils_registry::session::{self, Scheme};
use nils_registry::store::{Error as StoreError, Insert, Param, Store};
use nils_registry::{Registry, time::now_iso};

use crate::policy::{Policy, Uids};
use crate::scrub::{self, Plan};
use crate::tags::Category;
use crate::uid::Remap;

/// What a release will write, as a predicate over the registry.
///
/// Every field narrows; an empty selection is everything. v0 has two shapes of
/// scope, a cohort name and a list of stack ids, in two callers; here both are
/// this.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selection {
    /// Subject codes.
    pub subjects: Vec<String>,
    /// Stacks holding this value on the disposition axis. Empty means the
    /// release's default, which is everything that is not `excluded`.
    pub dispositions: Vec<String>,
    /// Stacks holding one of these roles.
    pub roles: Vec<String>,
    /// Only the stacks a pick chose (§10).
    pub picked_only: bool,
    pub modality: Option<String>,
}

impl Selection {
    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "subjects": self.subjects,
            "dispositions": self.dispositions,
            "roles": self.roles,
            "picked_only": self.picked_only,
            "modality": self.modality,
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
    pub root: String,
    pub policy: String,
    pub subjects: i64,
    pub stacks: i64,
    pub files: i64,
    pub bytes: i64,
    /// Files a reader could not read or a writer could not write, by reason.
    pub refused: BTreeMap<String, i64>,
    /// Every change, by tag and action, without a value anywhere.
    pub changes: BTreeMap<String, i64>,
    pub seconds: f64,
}

/// What one release needs beyond its policy.
pub struct Settings<'a> {
    pub name: &'a str,
    pub root: &'a Path,
    pub policy: &'a Policy,
    pub categories: Vec<Category>,
    pub selection: Selection,
    pub scheme: &'a Scheme,
    pub actor: &'a str,
    /// The key the pseudonyms and the UID remapping are derived from.
    pub key: &'a [u8],
    pub pack: &'a str,
    pub pack_version: &'a str,
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
    subject: i64,
    code: String,
    study: i64,
    path: String,
}

/// Run one release.
pub fn run(registry: &mut Registry, settings: &Settings) -> Result<Report, Error> {
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

    let mut report = Report {
        name: settings.name.to_string(),
        root: settings.root.display().to_string(),
        policy: settings.policy.describe(),
        ..Report::default()
    };
    let remap = match settings.policy.uids {
        Uids::Remap => Some(Remap::new(settings.policy.root.clone(), settings.key)),
        Uids::Preserve => None,
    };

    let instances = select(registry.store(), &settings.selection)?;
    let days = study_days(registry.store())?;
    report.release_id = open_row(registry.store(), settings)?;

    // Which studies are one occasion, per subject, so a file lands in the
    // session it belongs to rather than in a directory named by its date.
    let mut by_subject: BTreeMap<i64, Vec<Instance>> = BTreeMap::new();
    for i in instances {
        by_subject.entry(i.subject).or_default().push(i);
    }
    report.subjects = by_subject.len() as i64;

    let mut files: Vec<Vec<Param>> = Vec::new();
    let mut stacks: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for (subject, mine) in &by_subject {
        let labels = session_labels(mine, &days, settings.scheme);
        let offset = match settings.policy.dates {
            crate::dates::Policy::Shift => {
                let o = crate::dates::draw(settings.key, *subject);
                remember_offset(registry, *subject, o)?;
                o
            }
            _ => crate::dates::Offset(0),
        };
        let code = mine.first().map(|i| i.code.clone()).unwrap_or_default();
        let plan = Plan {
            policy: settings.policy,
            categories: &settings.categories,
            code: &code,
            offset,
            remap: remap.as_ref(),
        };
        for instance in mine {
            stacks.insert(instance.stack);
            let label = labels
                .get(&instance.study)
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());
            match write_one(instance, &plan, settings.root, &code, &label) {
                Ok(written) => {
                    report.files += 1;
                    report.bytes += written.bytes;
                    for ((tag, action), n) in &written.applied.changes {
                        *report.changes.entry(format!("{tag} {action}")).or_insert(0) += n;
                    }
                    files.push(vec![
                        Param::Int(report.release_id),
                        Param::Int(instance.id),
                        Param::from(written.path.as_str()),
                        Param::from(written.digest.as_str()),
                        Param::Int(written.bytes),
                    ]);
                }
                Err(why) => {
                    // By the reason and never by the path: a report that names
                    // the files it could not read names the source tree, which
                    // is the one thing a released dataset must not carry.
                    *report.refused.entry(why).or_insert(0) += 1;
                }
            }
        }
    }
    report.stacks = stacks.len() as i64;

    write_files(registry.store(), &files)?;
    close_row(registry.store(), &report)?;
    report.seconds = started.elapsed().as_secs_f64();
    Ok(report)
}

/// Every instance the selection names, with what the writer needs.
fn select(store: &mut Store, selection: &Selection) -> Result<Vec<Instance>, Error> {
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
    if let Some(m) = &selection.modality {
        wheres.push(format!("se.modality = '{}'", m.replace('\'', "''")));
    }
    let axis = store.qualified("classification_axis");
    // A stack the pack ruled out is not written, and saying so as a default
    // rather than as a flag is what keeps a release from carrying screenshots.
    let dispositions = if selection.dispositions.is_empty() {
        vec!["excluded".to_string()]
    } else {
        selection.dispositions.clone()
    };
    if selection.dispositions.is_empty() {
        wheres.push(format!(
            "NOT EXISTS (SELECT 1 FROM {axis} a WHERE a.stack_id = k.id AND a.axis = 'disposition' \
             AND a.value = 'excluded')"
        ));
    } else {
        wheres.push(format!(
            "EXISTS (SELECT 1 FROM {axis} a WHERE a.stack_id = k.id AND a.axis = 'disposition' \
             AND a.value IN ({}))",
            dispositions
                .iter()
                .map(|d| format!("'{}'", d.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !selection.roles.is_empty() {
        let any = selection
            .roles
            .iter()
            .map(|r| {
                format!(
                    "a.value = '{r}' OR a.value LIKE '{r},%' OR a.value LIKE '%,{r}' \
                     OR a.value LIKE '%,{r},%'",
                    r = r.replace('\'', "''")
                )
            })
            .collect::<Vec<_>>()
            .join(" OR ");
        wheres.push(format!(
            "EXISTS (SELECT 1 FROM {axis} a WHERE a.stack_id = k.id AND a.axis = 'role' \
             AND ({any}))"
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
    let filter = if wheres.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", wheres.join(" AND "))
    };
    let sql = format!(
        // The stored path is relative to the batch root it was walked from,
        // which is what makes a registry portable; the root is on `source`.
        "SELECT i.id, k.id, se.subject_id, su.code, se.study_id, so.root, sf.path \
         FROM {instance} i \
         JOIN {stack} k ON k.id = i.stack_id \
         JOIN {series} se ON se.id = i.series_id \
         JOIN {subject} su ON su.id = se.subject_id \
         JOIN {source_file} sf ON sf.instance_id = i.id \
         JOIN {source} so ON so.id = sf.source_id{filter} \
         ORDER BY se.subject_id, se.study_id, k.id, i.id",
        instance = store.qualified("instance"),
        stack = store.qualified("stack"),
        series = store.qualified("series"),
        subject = store.qualified("subject"),
        source_file = store.qualified("source_file"),
        source = store.qualified("source"),
    );
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for r in store.query(&sql, &[])? {
        let id = r.int(0)?;
        // An instance may sit under more than one path (§ Wave 1's duplicate
        // handling); one copy of it is written.
        if !seen.insert(id) {
            continue;
        }
        out.push(Instance {
            id,
            stack: r.int(1)?,
            subject: r.int(2)?,
            code: r.text(3)?.to_string(),
            study: r.int(4)?,
            path: Path::new(r.text(5)?).join(r.text(6)?).display().to_string(),
        });
    }
    Ok(out)
}

fn study_days(store: &mut Store) -> Result<HashMap<i64, Day>, Error> {
    let sql = format!(
        "SELECT id, COALESCE(date_filled, study_date) FROM {} \
         WHERE COALESCE(date_filled, study_date) IS NOT NULL",
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
    path: String,
    digest: String,
    bytes: i64,
    applied: scrub::Applied,
}

/// Read one file whole, apply the plan, write it out.
///
/// Read whole, because a release writes the pixels. The digest reader stops
/// before them, which is right for reading a header at speed and wrong here.
fn write_one(
    instance: &Instance,
    plan: &Plan,
    root: &Path,
    code: &str,
    label: &str,
) -> Result<Written, String> {
    let mut object = open(Path::new(&instance.path))?;
    let applied = scrub::apply(&mut object, plan);

    let relative = PathBuf::from(format!("sub-{code}"))
        .join(format!("ses-{label}"))
        .join(format!("{:08}", instance.stack))
        .join(format!("{:08}.dcm", instance.id));
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
        path: relative.display().to_string(),
        digest,
        bytes: bytes.len() as i64,
        applied,
    })
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

fn open_row(store: &mut Store, settings: &Settings) -> Result<i64, Error> {
    let categories: Vec<&str> = settings.categories.iter().map(|c| c.name()).collect();
    let written = store.insert(
        &Insert::new(
            table("release"),
            &[
                "name",
                "root",
                "policy",
                "selection",
                "categories",
                "session_scheme",
                "pack",
                "pack_version",
                "actor",
                "started_at",
                "files",
                "subjects",
            ],
        )
        .returning(&["id"]),
        &[vec![
            Param::from(settings.name),
            Param::from(settings.root.display().to_string()),
            Param::from(settings.policy.as_json().to_string()),
            Param::from(settings.selection.as_json().to_string()),
            Param::from(categories.join(",")),
            Param::from(
                serde_json::to_string(settings.scheme).unwrap_or_else(|_| "{}".to_string()),
            ),
            Param::from(settings.pack),
            Param::from(settings.pack_version),
            Param::from(settings.actor),
            Param::from(now_iso()),
            Param::Int(0),
            Param::Int(0),
        ]],
    )?;
    Ok(written.first().map(|r| r.int(0)).transpose()?.unwrap_or(0))
}

fn write_files(store: &mut Store, files: &[Vec<Param>]) -> Result<(), Error> {
    if files.is_empty() {
        return Ok(());
    }
    store.begin()?;
    let result = store.insert(
        &Insert::new(
            table("release_file"),
            &["release_id", "instance_id", "path", "digest", "bytes"],
        ),
        files,
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

fn close_row(store: &mut Store, report: &Report) -> Result<(), Error> {
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET finished_at = {}, files = {}, subjects = {} WHERE id = {}",
        store.qualified("release"),
        d.param(1, Type::Timestamp),
        d.param(2, Type::Int),
        d.param(3, Type::Int),
        d.param(4, Type::Int),
    );
    store.execute(
        &sql,
        &[
            Param::from(now_iso()),
            Param::Int(report.files),
            Param::Int(report.subjects),
            Param::Int(report.release_id),
        ],
    )?;
    Ok(())
}
