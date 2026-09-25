// SPDX-License-Identifier: AGPL-3.0-only

//! The report of a run (`docs/specs/wave1-parse-and-digest.md`, §10 and §11):
//! counts per quarantine class and per diagnostic kind, what was parsed, the
//! rate. Nothing in it comes out of a file but SOP class and transfer syntax
//! UIDs, modality and character set codes, tag keywords and the reader's error
//! texts; the diagnostic samples are shapes.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::hash::{DefaultHasher, Hash, Hasher};

use nils_dicom::extract::sop_class_name;
use nils_dicom::refusal::{set_aside_kind, sop_class_kind, sop_class_label};
use nils_dicom::{Diagnostic, DiagnosticKind, Extracted, QuarantineClass, Refusal};
use serde::{Deserialize, Serialize};

use crate::cancel::Cancelled;
use crate::stack::FileStack;
use crate::walk::SkipReason;

/// How many distinct samples a diagnostic kind keeps.
pub const SAMPLE_MAX: usize = 10;

/// What one parser thread tallies; the threads' counts are merged into the
/// report at the end.
#[derive(Debug, Default)]
pub struct Counts {
    /// Candidate files handed to the parser.
    pub seen: u64,
    pub parsed: u64,
    pub quarantined: u64,
    /// Candidate files an earlier run recorded and this one found unchanged
    /// (§5.2): seen, not read.
    pub unchanged: u64,
    /// Regular files the `files` knob did not select.
    pub filtered: u64,
    pub symlinks: u64,
    pub special: u64,
    pub walk_errors: u64,
    /// The size of every candidate file.
    pub bytes: u64,
    quarantine: BTreeMap<QuarantineClass, u64>,
    breakdown: BTreeMap<QuarantineClass, BTreeMap<String, u64>>,
    studies: HashSet<u64>,
    series: HashSet<u64>,
    subjects: HashSet<u64>,
    /// Stacks as `(series, key)` pairs (§8).
    stacks: HashSet<u64>,
    /// Frames read, one per classic instance and one per frame of an
    /// enhanced multi-frame object (record 37, S8).
    frames: u64,
    /// Files whose frames made more than one stack.
    multi_stack_files: u64,
    modalities: BTreeMap<String, u64>,
    sop_classes: BTreeMap<String, u64>,
    transfer_syntaxes: BTreeMap<String, u64>,
    charsets: BTreeMap<String, u64>,
    forms: BTreeMap<&'static str, u64>,
    diagnostics: BTreeMap<DiagnosticKind, Tally>,
    /// How many files the rule's first field source gave a value for, and the
    /// distinct values among them, hashed and capped. A batch whose whole
    /// archive reads one value there is looking at a placeholder, and no
    /// single file can see that (§3.3).
    identity_probed: u64,
    identity_values: HashSet<u64>,
    /// The shape of the first value seen, which is what the sample carries.
    identity_shape: Option<String>,
}

/// Beyond this many distinct values the field is plainly not a constant, and
/// the set stops growing.
const PROBE_MAX: usize = 64;

/// Below this many probed files a batch is too small to call a value constant.
pub(crate) const PROBE_MIN: u64 = 20;

#[derive(Debug, Default, Clone)]
struct Tally {
    count: u64,
    samples: BTreeSet<String>,
}

impl Tally {
    fn add(&mut self, sample: String) {
        self.count += 1;
        if self.samples.len() < SAMPLE_MAX {
            self.samples.insert(sample);
        }
    }
}

fn hash_of(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

impl Counts {
    /// A file the reader accepted, filed under `value` of `id_type` (§7.3)
    /// and in the stacks `stacks` of its series (§8): one, unless the frames
    /// of an enhanced object state more (record 37, S8).
    pub fn accepted(
        &mut self,
        x: &Extracted,
        id_type: &str,
        value: &str,
        stacks: &[FileStack],
        bytes: u64,
    ) {
        self.seen += 1;
        self.parsed += 1;
        self.bytes += bytes;
        self.studies.insert(hash_of(&x.study_uid));
        self.series.insert(hash_of(&x.series_uid));
        self.subjects
            .insert(hash_of(&format!("{id_type}\0{value}")));
        for s in stacks {
            self.stacks
                .insert(hash_of(&format!("{}\0{}", x.series_uid, s.signature.key)));
        }
        self.frames += u64::from(x.frames.count.max(1));
        if stacks.len() > 1 {
            self.multi_stack_files += 1;
        }
        *self.modalities.entry(x.modality.clone()).or_default() += 1;
        *self.sop_classes.entry(x.sop_class.clone()).or_default() += 1;
        *self
            .transfer_syntaxes
            .entry(x.transfer_syntax.clone())
            .or_default() += 1;
        let charset = x
            .charset
            .declared
            .clone()
            .unwrap_or_else(|| "(none)".into());
        *self.charsets.entry(charset).or_default() += 1;
        *self.forms.entry(x.form.name()).or_default() += 1;
        for d in &x.diagnostics {
            self.diagnostic(d);
        }
    }

    /// What the rule's first field source read on one file, for the constant
    /// check. `None` is a file where that source had nothing.
    pub fn probe_identity(&mut self, value: Option<&str>) {
        let Some(value) = value else { return };
        self.identity_probed += 1;
        if self.identity_values.len() < PROBE_MAX {
            self.identity_values.insert(hash_of(value));
        }
        if self.identity_shape.is_none() {
            self.identity_shape = Some(nils_dicom::diagnostic::shape(value));
        }
    }

    /// The diagnostic no file can raise: one value covering a whole batch.
    /// Called once, when the threads' counts have been merged.
    pub fn batch_diagnostics(&mut self) {
        if self.identity_probed >= PROBE_MIN && self.identity_values.len() == 1 {
            let shape = self.identity_shape.clone().unwrap_or_default();
            let d = Diagnostic::new(DiagnosticKind::IdentityConstant, "the rule's first field");
            let sample = match shape.is_empty() {
                true => d.sample(),
                false => format!("{}={shape}", d.subject),
            };
            let tally = self
                .diagnostics
                .entry(DiagnosticKind::IdentityConstant)
                .or_default();
            tally.count = self.identity_probed;
            tally.samples.insert(sample);
        }
    }

    /// A file the reader refused.
    pub fn refused(&mut self, r: &Refusal, bytes: u64) {
        self.seen += 1;
        self.quarantined += 1;
        self.bytes += bytes;
        *self.quarantine.entry(r.class).or_default() += 1;
        if let Some(key) = breakdown_key(r) {
            *self
                .breakdown
                .entry(r.class)
                .or_default()
                .entry(key)
                .or_default() += 1;
        }
    }

    /// A file found as an earlier run recorded it.
    pub fn unchanged(&mut self) {
        self.seen += 1;
        self.unchanged += 1;
    }

    /// An entry the walker did not hand on.
    pub fn skipped(&mut self, reason: SkipReason) {
        match reason {
            SkipReason::Symlink => self.symlinks += 1,
            SkipReason::Special => self.special += 1,
            SkipReason::Filtered => self.filtered += 1,
        }
    }

    /// A directory the walker could not list; the sample is the error's text,
    /// never the path.
    pub fn walk_error(&mut self, error: &str) {
        self.walk_errors += 1;
        self.diagnostics
            .entry(DiagnosticKind::WalkError)
            .or_default()
            .add(error.to_string());
    }

    pub fn diagnostic(&mut self, d: &Diagnostic) {
        self.diagnostics.entry(d.kind).or_default().add(d.sample());
    }

    /// Every diagnostic kind counted, with its samples: what the writer
    /// records per batch.
    pub fn diagnostic_rows(
        &self,
    ) -> impl Iterator<Item = (DiagnosticKind, u64, &BTreeSet<String>)> {
        self.diagnostics
            .iter()
            .map(|(kind, t)| (*kind, t.count, &t.samples))
    }

    /// Fold another thread's counts into these.
    pub fn merge(&mut self, other: Counts) {
        self.seen += other.seen;
        self.parsed += other.parsed;
        self.quarantined += other.quarantined;
        self.unchanged += other.unchanged;
        self.filtered += other.filtered;
        self.symlinks += other.symlinks;
        self.special += other.special;
        self.walk_errors += other.walk_errors;
        self.bytes += other.bytes;
        self.frames += other.frames;
        self.multi_stack_files += other.multi_stack_files;
        for (k, v) in other.quarantine {
            *self.quarantine.entry(k).or_default() += v;
        }
        for (class, keys) in other.breakdown {
            let mine = self.breakdown.entry(class).or_default();
            for (k, v) in keys {
                *mine.entry(k).or_default() += v;
            }
        }
        self.identity_probed += other.identity_probed;
        for v in other.identity_values {
            if self.identity_values.len() < PROBE_MAX {
                self.identity_values.insert(v);
            }
        }
        if self.identity_shape.is_none() {
            self.identity_shape = other.identity_shape;
        }
        self.studies.extend(other.studies);
        self.series.extend(other.series);
        self.subjects.extend(other.subjects);
        self.stacks.extend(other.stacks);
        merge_map(&mut self.modalities, other.modalities);
        merge_map(&mut self.sop_classes, other.sop_classes);
        merge_map(&mut self.transfer_syntaxes, other.transfer_syntaxes);
        merge_map(&mut self.charsets, other.charsets);
        merge_map(&mut self.forms, other.forms);
        for (kind, tally) in other.diagnostics {
            let mine = self.diagnostics.entry(kind).or_default();
            mine.count += tally.count;
            for s in tally.samples {
                if mine.samples.len() >= SAMPLE_MAX {
                    break;
                }
                mine.samples.insert(s);
            }
        }
    }

    /// The count of one quarantine class.
    pub fn class(&self, class: QuarantineClass) -> u64 {
        self.quarantine.get(&class).copied().unwrap_or(0)
    }

    /// The count of one diagnostic kind.
    pub fn kind(&self, kind: DiagnosticKind) -> u64 {
        self.diagnostics.get(&kind).map(|t| t.count).unwrap_or(0)
    }
}

fn merge_map<K: Ord>(mine: &mut BTreeMap<K, u64>, other: BTreeMap<K, u64>) {
    for (k, v) in other {
        *mine.entry(k).or_default() += v;
    }
}

/// What a refusal is counted by inside its class.
fn breakdown_key(r: &Refusal) -> Option<String> {
    match r.class {
        QuarantineClass::NotDicom => None,
        QuarantineClass::ParseError => Some(
            r.detail
                .as_deref()
                .and_then(|d| d.split_once(": ").map(|(kind, _)| kind))
                .unwrap_or("(unknown)")
                .to_string(),
        ),
        QuarantineClass::MissingModality => {
            Some(r.detail.clone().unwrap_or_else(|| "(absent)".into()))
        }
        _ => r.detail.clone(),
    }
}

/// What a run set aside, by kind (record 37, S9). Every quarantined file is
/// in exactly one kind: the SOP classes by their family, the rest by what the
/// class itself means.
fn set_aside(counts: &Counts) -> Vec<SetAside> {
    let mut kinds: BTreeMap<&'static str, (u64, BTreeMap<String, u64>)> = BTreeMap::new();
    for &class in &QuarantineClass::ALL {
        let total = counts.class(class);
        if total == 0 {
            continue;
        }
        let breakdown = counts.breakdown.get(&class);
        if class == QuarantineClass::UnsupportedSopClass {
            // one kind per family, the classes inside it named
            let mut named = 0;
            for (uid, n) in breakdown.into_iter().flatten() {
                let entry = kinds.entry(sop_class_kind(uid)).or_default();
                entry.0 += n;
                *entry.1.entry(sop_class_label(uid)).or_default() += n;
                named += n;
            }
            // a file whose refusal carried no UID cannot be named further
            if total > named {
                kinds.entry("a class with no UID").or_default().0 += total - named;
            }
            continue;
        }
        let entry = kinds.entry(set_aside_kind(class, None)).or_default();
        entry.0 += total;
        for (key, n) in breakdown.into_iter().flatten() {
            *entry.1.entry(key.clone()).or_default() += n;
        }
    }
    let mut out: Vec<SetAside> = kinds
        .into_iter()
        .map(|(kind, (count, classes))| SetAside {
            kind: kind.to_string(),
            count,
            classes: keyed(&classes),
        })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.kind.cmp(&b.kind)));
    out
}

/// A count keyed by a text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Keyed {
    pub key: String,
    pub count: u64,
}

fn keyed<K: ToString>(map: &BTreeMap<K, u64>) -> Vec<Keyed> {
    let mut v: Vec<Keyed> = map
        .iter()
        .map(|(k, c)| Keyed {
            key: k.to_string(),
            count: *c,
        })
        .collect();
    v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));
    v
}

/// One quarantine class in the report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassCount {
    pub class: String,
    pub count: u64,
    pub breakdown: Vec<Keyed>,
}

impl ClassCount {
    /// The kinds of object this class set aside, with their counts
    /// (record 37, S9): the families of the SOP classes it refused, or the
    /// one kind the class itself means. What the review item carries as its
    /// evidence, so a queue says what was left behind and not only how much.
    pub fn kinds(&self) -> Vec<Keyed> {
        let class = QuarantineClass::ALL
            .iter()
            .copied()
            .find(|c| c.name() == self.class);
        let Some(class) = class else {
            return Vec::new();
        };
        if class != QuarantineClass::UnsupportedSopClass {
            return vec![Keyed {
                key: set_aside_kind(class, None).to_string(),
                count: self.count,
            }];
        }
        let mut kinds: BTreeMap<String, u64> = BTreeMap::new();
        let mut named = 0;
        for k in &self.breakdown {
            *kinds.entry(sop_class_kind(&k.key).to_string()).or_default() += k.count;
            named += k.count;
        }
        if self.count > named {
            *kinds.entry("a class with no UID".to_string()).or_default() += self.count - named;
        }
        keyed(&kinds)
    }
}

/// One kind of object a run set aside, with the classes inside it
/// (record 37, S9): what the archive held and NILS did not take.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetAside {
    /// The family in words: `secondary capture`, `presentation state`, `not
    /// DICOM`.
    pub kind: String,
    pub count: u64,
    /// What the kind was made of: the standard's name for each SOP class
    /// where NILS knows it, else the UID; for the other quarantine classes,
    /// the class's own breakdown (a modality code, a reader's error kind).
    pub classes: Vec<Keyed>,
}

/// One SOP class in the report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SopClassCount {
    pub uid: String,
    pub name: Option<String>,
    pub count: u64,
}

/// One diagnostic kind in the report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticCount {
    pub kind: String,
    pub count: u64,
    pub samples: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skipped {
    pub symlink: u64,
    pub special: u64,
}

/// The settings the report repeats.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Setup {
    pub name: String,
    pub root: String,
    pub dry_run: bool,
    pub files: String,
    pub workers: usize,
    pub walk_threads: usize,
    /// Which private elements were read, and from which pack: `none` when
    /// no pack was given (Wave 4a §5.2).
    #[serde(default = "no_private")]
    pub private: String,
}

fn no_private() -> String {
    "none".to_string()
}

/// What the writer did (§9.1): the rows of a run that was not a dry run.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Written {
    pub batch_id: i64,
    /// The epoch after the last write.
    pub epoch: i64,
    /// Transactions committed.
    pub writes: u64,
    /// Files filed as their instance's own (§5.3).
    pub ingested: u64,
    /// Files whose instance another file holds.
    pub duplicate: u64,
    /// Files read again because their size or time differed from their record.
    pub changed: u64,
    /// Files an earlier run quarantined, left as they were.
    pub quarantine_kept: u64,
    /// Record 26 §4: files held for want of a map, quarantined under
    /// `identity.unmapped` instead of filed, where the dataset says `hold`.
    #[serde(default)]
    pub held: u64,
    /// Records marked gone at the end of the walk.
    pub gone: u64,
    pub subjects_created: u64,
    /// Identifiers the linkage store had met (§7.4 step 3).
    #[serde(default)]
    pub subjects_matched: u64,
    /// Identifiers attached to a subject found by its code (§7.4 step 5).
    #[serde(default)]
    pub identities_attached: u64,
    pub studies_created: u64,
    pub series_created: u64,
    pub stacks_created: u64,
    /// Record 37 S8: rows written saying which frames of an instance are in
    /// which stack, for the files whose frames made more than one.
    #[serde(default)]
    pub frame_groups: u64,
    /// Stacks and series the run made and removed at its end because they
    /// hold no instance: every file of theirs was a duplicate of an
    /// instance held under another series. Not counted in the created.
    #[serde(default)]
    pub empty_stacks_removed: u64,
    #[serde(default)]
    pub empty_series_removed: u64,
}

/// Record 26 §8: what feeding the dataset's cohort did. Every subject the
/// batch created joined, and so did one it met whose membership was not
/// open; `refused` says why nothing was written when the name could not be
/// a cohort's.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Joined {
    pub cohort: String,
    /// Intervals opened.
    pub subjects: u64,
    /// Subjects the batch created or met, members already or not.
    pub met: u64,
    /// The cohort was made on this first use.
    pub created: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}

/// The report: the JSON of `--json`, the text otherwise.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    #[serde(flatten)]
    pub setup: Setup,
    pub elapsed_s: f64,
    pub files_per_s: f64,
    pub mb_per_s: f64,
    pub peak_rss_bytes: Option<u64>,
    pub seen: u64,
    pub parsed: u64,
    pub quarantined: u64,
    #[serde(default)]
    pub unchanged: u64,
    pub filtered: u64,
    pub skipped: Skipped,
    pub walk_errors: u64,
    pub bytes: u64,
    pub quarantine: Vec<ClassCount>,
    pub studies: u64,
    pub series: u64,
    pub subjects: u64,
    /// Distinct `(series, stack key)` pairs among the parsed files (§8).
    #[serde(default)]
    pub stacks: u64,
    /// Frames read (record 37, S8): one per classic instance, one per frame
    /// of an enhanced multi-frame object.
    #[serde(default)]
    pub frames: u64,
    /// Files whose frames held more than one stack.
    #[serde(default)]
    pub multi_stack_files: u64,
    /// Record 37 S9: what was set aside, by kind and count.
    #[serde(default)]
    pub set_aside: Vec<SetAside>,
    pub modalities: Vec<Keyed>,
    pub sop_classes: Vec<SopClassCount>,
    pub transfer_syntaxes: Vec<Keyed>,
    pub charsets: Vec<Keyed>,
    pub forms: Vec<Keyed>,
    pub diagnostics: Vec<DiagnosticCount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written: Option<Written>,
    /// Record 26 §8: the cohort the dataset feeds, and what joined it;
    /// absent when the tree is no dataset's or the dataset feeds none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub joined: Option<Joined>,
    /// How the run ended when it was asked to stop (§10): absent when it ran
    /// to the end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancelled: Option<Cancelled>,
}

impl Report {
    pub fn new(
        setup: Setup,
        counts: &Counts,
        elapsed_s: f64,
        peak_rss_bytes: Option<u64>,
    ) -> Report {
        let rate = |n: f64| if elapsed_s > 0.0 { n / elapsed_s } else { 0.0 };
        let mut sop_classes: Vec<SopClassCount> = counts
            .sop_classes
            .iter()
            .map(|(uid, c)| SopClassCount {
                uid: uid.clone(),
                name: sop_class_name(uid).map(str::to_string),
                count: *c,
            })
            .collect();
        sop_classes.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.uid.cmp(&b.uid)));
        Report {
            setup,
            elapsed_s,
            files_per_s: rate(counts.seen as f64),
            mb_per_s: rate(counts.bytes as f64 / 1e6),
            peak_rss_bytes,
            seen: counts.seen,
            parsed: counts.parsed,
            quarantined: counts.quarantined,
            unchanged: counts.unchanged,
            filtered: counts.filtered,
            skipped: Skipped {
                symlink: counts.symlinks,
                special: counts.special,
            },
            walk_errors: counts.walk_errors,
            bytes: counts.bytes,
            quarantine: QuarantineClass::ALL
                .iter()
                .map(|&class| ClassCount {
                    class: class.name().to_string(),
                    count: counts.class(class),
                    breakdown: counts.breakdown.get(&class).map(keyed).unwrap_or_default(),
                })
                .collect(),
            set_aside: set_aside(counts),
            studies: counts.studies.len() as u64,
            series: counts.series.len() as u64,
            subjects: counts.subjects.len() as u64,
            stacks: counts.stacks.len() as u64,
            frames: counts.frames,
            multi_stack_files: counts.multi_stack_files,
            modalities: keyed(&counts.modalities),
            sop_classes,
            transfer_syntaxes: keyed(&counts.transfer_syntaxes),
            charsets: keyed(&counts.charsets),
            forms: keyed(&counts.forms),
            diagnostics: DiagnosticKind::ALL
                .iter()
                .filter_map(|&kind| {
                    counts.diagnostics.get(&kind).map(|t| DiagnosticCount {
                        kind: kind.name().to_string(),
                        count: t.count,
                        samples: t.samples.iter().cloned().collect(),
                    })
                })
                .collect(),
            written: None,
            joined: None,
            cancelled: None,
        }
    }

    /// The count of one quarantine class.
    pub fn class(&self, name: &str) -> u64 {
        self.quarantine
            .iter()
            .find(|c| c.class == name)
            .map(|c| c.count)
            .unwrap_or(0)
    }

    /// The count of one diagnostic kind.
    pub fn kind(&self, name: &str) -> u64 {
        self.diagnostics
            .iter()
            .find(|d| d.kind == name)
            .map(|d| d.count)
            .unwrap_or(0)
    }
}

/// `1234567` as `1,234,567`.
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A byte count in decimal units.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Seconds as `7.9 s`, `2 min 13 s` or `1 h 02 min`.
pub fn human_secs(s: f64) -> String {
    if s < 60.0 {
        format!("{s:.1} s")
    } else if s < 3600.0 {
        format!("{} min {:02} s", (s / 60.0) as u64, (s % 60.0) as u64)
    } else {
        format!(
            "{} h {:02} min",
            (s / 3600.0) as u64,
            ((s % 3600.0) / 60.0) as u64
        )
    }
}

fn keyed_line(f: &mut fmt::Formatter<'_>, label: &str, items: &[Keyed]) -> fmt::Result {
    if items.is_empty() {
        return Ok(());
    }
    write!(f, "  {label:<17}")?;
    for (i, k) in items.iter().enumerate() {
        if i > 0 {
            f.write_str("   ")?;
        }
        write!(f, "{} {}", k.key, thousands(k.count))?;
    }
    writeln!(f)
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = &self.setup;
        let ended = match (s.dry_run, self.cancelled) {
            (true, None) => " (dry run)",
            (true, Some(Cancelled::Stopped)) => " (dry run, stopped)",
            (true, Some(Cancelled::Aborted)) => " (dry run, aborted)",
            (false, Some(Cancelled::Stopped)) => " (stopped)",
            (false, Some(Cancelled::Aborted)) => " (aborted)",
            (false, None) => "",
        };
        writeln!(f, "nils digest{ended}   name {}   root {}", s.name, s.root)?;
        writeln!(
            f,
            "  files            {} seen   {} parsed   {} quarantined   {} unchanged   {} filtered   {} skipped ({} symlink, {} special)   {} walk errors",
            thousands(self.seen),
            thousands(self.parsed),
            thousands(self.quarantined),
            thousands(self.unchanged),
            thousands(self.filtered),
            thousands(self.skipped.symlink + self.skipped.special),
            thousands(self.skipped.symlink),
            thousands(self.skipped.special),
            thousands(self.walk_errors),
        )?;
        write!(
            f,
            "  run              {} in {}   {} files/s   {:.1} MB/s   workers {}   walk threads {}   files {}",
            human_bytes(self.bytes),
            human_secs(self.elapsed_s),
            thousands(self.files_per_s.round() as u64),
            self.mb_per_s,
            s.workers,
            s.walk_threads,
            s.files,
        )?;
        match self.peak_rss_bytes {
            Some(b) => writeln!(f, "   peak RSS {}", human_bytes(b))?,
            None => writeln!(f)?,
        }
        writeln!(f, "  private elements {}", s.private)?;

        if let Some(w) = &self.written {
            writeln!(f, "written")?;
            writeln!(
                f,
                "  batch {}   epoch {}   {} writes   {} ingested   {} duplicate   {} changed   {} quarantine kept   {} gone",
                w.batch_id,
                w.epoch,
                thousands(w.writes),
                thousands(w.ingested),
                thousands(w.duplicate),
                thousands(w.changed),
                thousands(w.quarantine_kept),
                thousands(w.gone),
            )?;
            writeln!(
                f,
                "  created          subjects {}   studies {}   series {}   stacks {}{}",
                thousands(w.subjects_created),
                thousands(w.studies_created),
                thousands(w.series_created),
                thousands(w.stacks_created),
                match w.frame_groups {
                    0 => String::new(),
                    n => format!("   frame groups {}", thousands(n)),
                },
            )?;
            if w.empty_stacks_removed + w.empty_series_removed > 0 {
                writeln!(
                    f,
                    "  removed empty    series {}   stacks {}   (every file of theirs a duplicate held under another series)",
                    thousands(w.empty_series_removed),
                    thousands(w.empty_stacks_removed),
                )?;
            }
            writeln!(
                f,
                "  identity         known {}   attached {}",
                thousands(w.subjects_matched),
                thousands(w.identities_attached),
            )?;
        }
        if let Some(j) = &self.joined {
            match &j.refused {
                Some(why) => writeln!(f, "  cohort {}   not fed: {why}", j.cohort)?,
                None => writeln!(
                    f,
                    "  cohort {}   joined {} of {} subject(s){}",
                    j.cohort,
                    thousands(j.subjects),
                    thousands(j.met),
                    if j.created { "   made now" } else { "" },
                )?,
            }
        }

        writeln!(f, "quarantine")?;
        for c in &self.quarantine {
            write!(f, "  {:<24} {:>9}", c.class, thousands(c.count))?;
            if !c.breakdown.is_empty() {
                f.write_str("   ")?;
                for (i, k) in c.breakdown.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{} {}", k.key, thousands(k.count))?;
                }
            }
            writeln!(f)?;
        }

        if !self.set_aside.is_empty() {
            writeln!(f, "set aside")?;
            for k in &self.set_aside {
                write!(f, "  {:<24} {:>9}", k.kind, thousands(k.count))?;
                if !k.classes.is_empty() {
                    f.write_str("   ")?;
                    for (i, c) in k.classes.iter().enumerate() {
                        if i > 0 {
                            f.write_str(", ")?;
                        }
                        write!(f, "{} {}", c.key, thousands(c.count))?;
                    }
                }
                writeln!(f)?;
            }
        }

        writeln!(f, "content")?;
        writeln!(
            f,
            "  studies {}   series {}   stacks {}   subjects {}",
            thousands(self.studies),
            thousands(self.series),
            thousands(self.stacks),
            thousands(self.subjects)
        )?;
        if self.frames > self.parsed {
            writeln!(
                f,
                "  frames {}   over more than one stack   {} file(s)",
                thousands(self.frames),
                thousands(self.multi_stack_files),
            )?;
        }
        keyed_line(f, "modality", &self.modalities)?;
        if !self.sop_classes.is_empty() {
            write!(f, "  {:<17}", "sop class")?;
            for (i, c) in self.sop_classes.iter().enumerate() {
                if i > 0 {
                    f.write_str("   ")?;
                }
                match &c.name {
                    Some(name) => write!(f, "{name} {}", thousands(c.count))?,
                    None => write!(f, "{} {}", c.uid, thousands(c.count))?,
                }
            }
            writeln!(f)?;
        }
        keyed_line(f, "transfer syntax", &self.transfer_syntaxes)?;
        keyed_line(f, "charset", &self.charsets)?;
        keyed_line(f, "form", &self.forms)?;

        writeln!(f, "diagnostics")?;
        if self.diagnostics.is_empty() {
            writeln!(f, "  none")?;
        }
        for d in &self.diagnostics {
            writeln!(
                f,
                "  {:<24} {:>9}   {}",
                d.kind,
                thousands(d.count),
                d.samples.join(", ")
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_well() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(508045), "508,045");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1_500), "1.5 kB");
        assert_eq!(human_bytes(123_400_000_000), "123.4 GB");
        assert_eq!(human_secs(7.94), "7.9 s");
        assert_eq!(human_secs(133.0), "2 min 13 s");
        assert_eq!(human_secs(3720.0), "1 h 02 min");
    }

    #[test]
    fn refusals_are_counted_by_class_and_key() {
        let mut a = Counts::default();
        a.refused(&Refusal::new(QuarantineClass::NotDicom, None), 1);
        a.refused(
            &Refusal::new(
                QuarantineClass::ParseError,
                Some("truncated: end of file".into()),
            ),
            2,
        );
        a.refused(
            &Refusal::new(
                QuarantineClass::ParseError,
                Some("truncated: elsewhere".into()),
            ),
            3,
        );
        let mut b = Counts::default();
        b.refused(
            &Refusal::new(QuarantineClass::MissingUid, Some("SOPInstanceUID".into())),
            4,
        );
        b.walk_error("Permission denied (os error 13)");
        b.skipped(SkipReason::Symlink);
        b.skipped(SkipReason::Filtered);
        a.merge(b);
        assert_eq!(a.seen, 4);
        assert_eq!(a.quarantined, 4);
        assert_eq!(a.bytes, 10);
        assert_eq!(a.class(QuarantineClass::ParseError), 2);
        assert_eq!(a.kind(DiagnosticKind::WalkError), 1);
        assert_eq!(a.symlinks, 1);
        assert_eq!(a.filtered, 1);

        let setup = Setup {
            name: "t".into(),
            root: "/r".into(),
            dry_run: true,
            files: "all".into(),
            workers: 2,
            walk_threads: 1,
            private: "none".into(),
        };
        let report = Report::new(setup, &a, 2.0, Some(1 << 30));
        assert_eq!(report.class("parse_error"), 2);
        assert_eq!(
            report.quarantine[2].breakdown,
            vec![Keyed {
                key: "truncated".into(),
                count: 2
            }]
        );
        assert_eq!(report.class("missing_uid"), 1);
        assert_eq!(report.kind("walk_error"), 1);
        assert_eq!(report.files_per_s, 2.0);
        let text = report.to_string();
        assert!(text.contains("nils digest (dry run)   name t   root /r"));
        let mut stopped = report.clone();
        stopped.setup.dry_run = false;
        stopped.cancelled = Some(Cancelled::Stopped);
        assert!(
            stopped
                .to_string()
                .starts_with("nils digest (stopped)   name t")
        );
        stopped.cancelled = Some(Cancelled::Aborted);
        assert!(
            stopped
                .to_string()
                .starts_with("nils digest (aborted)   name t")
        );
        stopped.cancelled = None;
        assert!(stopped.to_string().starts_with("nils digest   name t"));
        assert!(text.contains(&format!("  {:<24} {:>9}   truncated 2\n", "parse_error", 2)));
        assert!(text.contains(&format!(
            "  {:<24} {:>9}   Permission denied (os error 13)\n",
            "walk_error", 1
        )));
        assert!(text.contains("peak RSS 1.1 GB"));
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
        assert_eq!(json["seen"], 4);
        assert_eq!(json["name"], "t");
        assert_eq!(json["quarantine"][0]["class"], "not_dicom");
        assert!(json.get("written").is_none());
        assert!(json.get("cancelled").is_none());

        // what `nils status --batch` prints comes back from the stored JSON
        let mut report = report;
        report.written = Some(Written {
            batch_id: 7,
            epoch: 3,
            writes: 2,
            ingested: 1,
            ..Written::default()
        });
        let text = serde_json::to_string(&report).unwrap();
        let back: Report = serde_json::from_str(&text).unwrap();
        assert_eq!(back.seen, 4);
        assert_eq!(back.class("parse_error"), 2);
        assert_eq!(back.written, report.written);
        assert!(
            back.to_string()
                .contains("  batch 7   epoch 3   2 writes   1 ingested")
        );
    }

    #[test]
    fn samples_are_capped_and_distinct() {
        let mut c = Counts::default();
        c.diagnostic(
            &Diagnostic::new(DiagnosticKind::ValueInvalid, "series_mr.echo_time").with_shape("9a"),
        );
        for i in 0..30 {
            c.diagnostic(
                &Diagnostic::new(DiagnosticKind::ValueInvalid, format!("study.col{i}"))
                    .with_shape("x"),
            );
            c.diagnostic(
                &Diagnostic::new(DiagnosticKind::ValueInvalid, "series_mr.echo_time")
                    .with_shape("9a"),
            );
        }
        let tally = &c.diagnostics[&DiagnosticKind::ValueInvalid];
        assert_eq!(tally.count, 61);
        assert_eq!(tally.samples.len(), SAMPLE_MAX);
        assert!(tally.samples.contains("series_mr.echo_time=9a"));
    }
}
