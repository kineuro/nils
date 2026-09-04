// SPDX-License-Identifier: AGPL-3.0-only

//! Write a corpus built to be broken, for Wave 3's repairs
//! (`docs/specs/wave3-anonymize-and-bids.md`, §12.1).
//!
//! ```sh
//! cargo run --release -p nils-dicom --example awkward -- \
//!     --out /scratch/nils/awkward > awkward-manifest.json
//! ```
//!
//! The `corpus` example beside this one writes a well-formed archive at scale.
//! This is its opposite: small, deliberately wrong, one directory per named
//! scenario, with a manifest that states the right answer for each so a gate
//! asserts instead of eyeballing.
//!
//! Nothing in it derives from any registry. Every value comes from the seed and
//! from the scenario table below, which is what lets the generator live in the
//! repository while the corpus lives in scratch (design record, C10).
//!
//! Eleven identity scenarios, eleven date scenarios and four session scenarios,
//! each salted with the mess a real tree carries: mixed and missing extensions,
//! a file that is not DICOM, a truncated file, a file with no SOP Instance UID,
//! an empty directory and a duplicate under a second path. A repair that works
//! only on a clean tree is not a repair.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, Elem, MetaFields};

/// The DICOM example root: every UID here is synthetic and says so.
const ROOT: &str = "1.2.826.0.1.3680043.8.498";
const MR_IMAGE: &str = "1.2.840.10008.5.1.4.1.1.4";

struct Options {
    out: PathBuf,
    seed: u64,
    /// Bytes of Pixel Data on every accepted file.
    pixel_bytes: usize,
}

fn usage() -> ! {
    eprintln!("usage: awkward --out DIR [--seed S] [--pixel-bytes B]");
    std::process::exit(2)
}

fn parse_args() -> Options {
    let mut out = None;
    let mut seed = 1u64;
    let mut pixel_bytes = 256usize;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || args.next().unwrap_or_else(|| usage());
        match a.as_str() {
            "--out" => out = Some(PathBuf::from(value())),
            "--seed" => seed = value().parse().unwrap_or_else(|_| usage()),
            "--pixel-bytes" => pixel_bytes = value().parse().unwrap_or_else(|_| usage()),
            _ => usage(),
        }
    }
    let Some(out) = out else { usage() };
    Options {
        out,
        seed,
        pixel_bytes,
    }
}

/// splitmix64, the same generator the `corpus` example uses.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
}

/// What a correct repair should say about one study.
struct StudyWant {
    /// Path of the study directory, relative to the corpus root.
    dir: String,
    /// The date a correct repair finds, or none when there is nothing to find.
    date: Option<&'static str>,
    /// Which source answered: `study_date`, `series_date`, `acquisition_date`,
    /// `content_date`, `uid`, or `none`.
    source: &'static str,
    /// A label for the person this study belongs to. It is a manifest label
    /// and never a subject code: it says which studies must land together.
    person: String,
}

/// One scenario: a subtree, what makes it awkward, and the right answer.
struct Scenario {
    name: &'static str,
    what: &'static str,
    /// How many distinct people the subtree describes.
    people: usize,
    /// What a reader must do to get that right.
    needs: &'static str,
    studies: Vec<StudyWant>,
}

/// Which path segment carries the subject code, counted from one within the
/// scenario's own directory. `None` means the code is in the file, so the
/// default rule reads it and no path source is needed.
///
/// It is per scenario because one rule cannot read every layout: an archive
/// whose subject folder sits under a site directory has to be told so, and
/// pretending otherwise lets a scenario pass for the wrong reason.
fn rule_of(name: &str) -> &'static str {
    match name {
        // the tag is the identity, which is v1's default rule
        "id-good" | "id-inconsistent" => "default",
        // the code hides in PatientName beside a date, so a pattern is the
        // only way to read it without taking the date too
        "id-in-name" => "name",
        // a site directory or a branch sits above the subject
        "id-depth2" | "id-two-paths" => "path:2",
        // and everywhere else the subject is the top directory
        n if n.starts_with("id-") => "path:1",
        // a date or session scenario says nothing about identity
        _ => "default",
    }
}

/// The diagnostic a scenario exists to provoke, when its point is that the
/// reader must speak rather than that it must be right.
fn diagnostic_of(name: &str) -> Option<&'static str> {
    match name {
        // v1 says it as a row-field disagreement on the study's subject,
        // which is the same observation under an older name.
        "id-inconsistent" => Some("field_disagreement"),
        "id-xxxx" | "id-constant" => Some("identity_constant"),
        _ => None,
    }
}

/// One instance, described by everything a scenario may want to bend.
#[derive(Clone)]
struct File {
    /// Relative path under the corpus root, extension included.
    path: String,
    patient_id: Option<&'static str>,
    patient_name: &'static str,
    study_uid: String,
    series_uid: String,
    sop: String,
    study_date: Option<&'static str>,
    series_date: Option<&'static str>,
    acquisition_date: Option<&'static str>,
    content_date: Option<&'static str>,
    /// `InstanceCreationDate`, which an anonymiser often rewrites to a first
    /// of January.
    instance_creation_date: Option<&'static str>,
    /// `PerformedProcedureStepStartDate`, which survives some scrubs.
    pps_start_date: Option<&'static str>,
    /// A Siemens CSA private element whose value carries a date, written under
    /// its private creator so a reader has to go looking for it.
    csa_version: Option<&'static str>,
    /// Written without a SOP Instance UID, which the reader refuses.
    no_sop: bool,
}

impl File {
    fn new(path: String, study_uid: String, series_uid: String, sop: String) -> Self {
        File {
            path,
            patient_id: Some("SUBJECT"),
            patient_name: "SYNTHETIC^SUBJECT",
            study_uid,
            series_uid,
            sop,
            study_date: Some("20220115"),
            series_date: Some("20220115"),
            acquisition_date: Some("20220115"),
            content_date: Some("20220115"),
            instance_creation_date: None,
            pps_start_date: None,
            csa_version: None,
            no_sop: false,
        }
    }
}

fn uid(parts: &[&str]) -> String {
    let mut s = String::from(ROOT);
    for p in parts {
        s.push('.');
        s.push_str(p);
    }
    s
}

fn elements(f: &File, pixel_bytes: usize) -> Vec<Elem> {
    let mut e = vec![synth::text(tags::SOP_CLASS_UID, VR::UI, MR_IMAGE)];
    if !f.no_sop {
        e.push(synth::text(tags::SOP_INSTANCE_UID, VR::UI, &f.sop));
    }
    e.push(synth::text(tags::STUDY_INSTANCE_UID, VR::UI, &f.study_uid));
    e.push(synth::text(
        tags::SERIES_INSTANCE_UID,
        VR::UI,
        &f.series_uid,
    ));
    e.push(synth::text(tags::MODALITY, VR::CS, "MR"));
    // A date is absent when the scenario says so: the element is not written,
    // which is what a scanner that never filled it in leaves behind.
    if let Some(v) = f.study_date {
        e.push(synth::text(tags::STUDY_DATE, VR::DA, v));
    }
    if let Some(v) = f.series_date {
        e.push(synth::text(tags::SERIES_DATE, VR::DA, v));
    }
    if let Some(v) = f.acquisition_date {
        e.push(synth::text(tags::ACQUISITION_DATE, VR::DA, v));
    }
    if let Some(v) = f.content_date {
        e.push(synth::text(tags::CONTENT_DATE, VR::DA, v));
    }
    if let Some(v) = f.instance_creation_date {
        e.push(synth::text(tags::INSTANCE_CREATION_DATE, VR::DA, v));
    }
    if let Some(v) = f.pps_start_date {
        e.push(synth::text(
            tags::PERFORMED_PROCEDURE_STEP_START_DATE,
            VR::DA,
            v,
        ));
    }
    if let Some(v) = f.csa_version {
        // The private creator claims the block, and the date rides inside a
        // version string, which is where Siemens leaves it.
        e.push(synth::text(
            dicom_core::Tag(0x0029, 0x0010),
            VR::LO,
            "SIEMENS CSA HEADER",
        ));
        e.push(synth::text(
            dicom_core::Tag(0x0029, 0x1008),
            VR::CS,
            "IMAGE NUM 4",
        ));
        e.push(synth::text(
            dicom_core::Tag(0x0029, 0x1009),
            VR::LO,
            &format!("syngo MR B17 {v}"),
        ));
    }
    if let Some(v) = f.patient_id {
        e.push(synth::text(tags::PATIENT_ID, VR::LO, v));
    }
    e.push(synth::text(tags::PATIENT_NAME, VR::PN, f.patient_name));
    e.push(synth::text(tags::SERIES_NUMBER, VR::IS, "1"));
    e.push(synth::text(
        tags::SERIES_DESCRIPTION,
        VR::LO,
        "ax t1 mprage",
    ));
    e.push(synth::text(tags::SCANNING_SEQUENCE, VR::CS, "GR"));
    e.push(synth::text(tags::SEQUENCE_VARIANT, VR::CS, "SK\\SP\\MP"));
    e.push(synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, "3D"));
    e.push(synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"));
    e.push(synth::text(
        tags::IMAGE_TYPE,
        VR::CS,
        "ORIGINAL\\PRIMARY\\M\\ND",
    ));
    e.push(synth::us(tags::ROWS, 64));
    e.push(synth::us(tags::COLUMNS, 64));
    e.push(synth::bytes(
        tags::PIXEL_DATA,
        VR::OW,
        vec![0u8; pixel_bytes],
    ));
    e
}

fn write(root: &Path, f: &File, pixel_bytes: usize) {
    let full = root.join(&f.path);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    let meta = MetaFields::mr(&f.sop);
    let bytes = synth::part10(&meta, &elements(f, pixel_bytes), true);
    fs::write(&full, bytes).expect("write");
}

/// The mess a real tree carries, added to every scenario so a repair is never
/// tested on a clean one.
fn sprinkle(root: &Path, rng: &mut Rng, _scenario: &str, one: &File, pixel_bytes: usize) {
    // Inside the seed's own study, never beside it: a `_mess` directory at the
    // subject level is indistinguishable from a subject, and a path-based rule
    // would read it as one.
    let study_dir = one
        .path
        .rsplit_once('/')
        .map(|(d, _)| d.to_string())
        .unwrap_or_default();
    let base = format!("{study_dir}/_mess");
    fs::create_dir_all(root.join(format!("{base}/empty-dir"))).expect("mkdir");

    // Not DICOM at all.
    fs::write(
        root.join(format!("{base}/notes.txt")),
        b"this is not a DICOM file\n",
    )
    .expect("write");

    // A DICOM cut off mid-element.
    let full = synth::part10(
        &MetaFields::mr(&uid(&["9", "0"])),
        &elements(one, pixel_bytes),
        true,
    );
    let cut = full.len() / 2;
    fs::write(root.join(format!("{base}/truncated.dcm")), &full[..cut]).expect("write");

    // Readable, but with nothing to identify the instance by.
    let mut headless = one.clone();
    headless.no_sop = true;
    headless.path = format!("{base}/no-sop-uid");
    write(root, &headless, pixel_bytes);

    // The same instance under a second path, which is a duplicate rather than
    // a second instance.
    let mut dup = one.clone();
    dup.path = format!("{base}/duplicate/{}", file_name(&one.path));
    write(root, &dup, pixel_bytes);

    // Extensions a walker must not depend on.
    for (i, ext) in ["", ".dcm", ".DCM", ".IMA"].iter().enumerate() {
        let mut f = one.clone();
        f.sop = uid(&["9", "1", &format!("{}", rng.below(1_000_000) + i as u64)]);
        f.path = format!("{base}/ext-{i}{ext}");
        write(root, &f, pixel_bytes);
    }
}

fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// Build one study of `count` instances under `dir`, bending `f` per file.
fn study(
    root: &Path,
    dir: &str,
    study_uid: &str,
    tag: &str,
    count: u64,
    pixel_bytes: usize,
    bend: impl Fn(&mut File, u64),
) -> File {
    let series_uid = uid(&[tag, "s1"]);
    let mut first = None;
    for i in 0..count {
        let sop = uid(&[tag, "i", &i.to_string()]);
        let mut f = File::new(
            format!("{dir}/{:04}.dcm", i + 1),
            study_uid.to_string(),
            series_uid.clone(),
            sop,
        );
        bend(&mut f, i);
        write(root, &f, pixel_bytes);
        if first.is_none() {
            first = Some(f);
        }
    }
    first.expect("at least one instance")
}

fn main() {
    let o = parse_args();
    let mut rng = Rng(o.seed.wrapping_mul(0x2545_F491_4F6C_DD1D) | 1);
    fs::create_dir_all(&o.out).expect("mkdir out");
    let root = o.out.as_path();
    let px = o.pixel_bytes;
    let mut scenarios: Vec<Scenario> = Vec::new();
    // One representative file per scenario: a scenario's mess is built from its
    // own instances, so it adds no subject and no study that the scenario did
    // not already declare.
    let mut seeds: Vec<(&'static str, File)> = Vec::new();

    // ---------------------------------------------------------------- identity

    // A well-formed baseline, so a failure elsewhere is the scenario and not
    // the generator.
    {
        let name = "id-good";
        let mut studies = Vec::new();
        for s in 1..=3u64 {
            let person = format!("P{s}");
            let code: &'static str = Box::leak(format!("SUBJ{s:03}").into_boxed_str());
            for v in 1..=2u64 {
                let dir = format!("{name}/{code}/visit{v}");
                let su = uid(&["1", &s.to_string(), &v.to_string()]);
                let f = study(root, &dir, &su, &format!("1-{s}-{v}"), 2, px, |f, _| {
                    f.patient_id = Some(code)
                });
                seeds.push((name, f));
                studies.push(StudyWant {
                    dir,
                    date: Some("20220115"),
                    source: "study_date",
                    person: person.clone(),
                });
            }
        }
        scenarios.push(Scenario {
            name,
            what: "everything present and consistent",
            people: 3,
            needs: "the PatientID tag alone",
            studies,
        });
    }

    // The case that started the wave: the tag is a placeholder and the code is
    // only in the path.
    for (name, id, what) in [
        (
            "id-xxxx",
            Some("XXXX"),
            "PatientID is the literal placeholder XXXX on every file",
        ),
        (
            "id-absent",
            None,
            "the PatientID element is not written at all",
        ),
        ("id-empty", Some(""), "PatientID is present and empty"),
        (
            "id-constant",
            Some("ANONYMOUS"),
            "PatientID is one constant for everyone",
        ),
    ] {
        let mut studies = Vec::new();
        for s in 1..=4u64 {
            let person = format!("P{s}");
            let folder = format!("{}{s:03}", name.to_uppercase().replace('-', ""));
            for v in 1..=3u64 {
                let dir = format!("{name}/{folder}/visit{v}");
                let su = uid(&["2", name, &s.to_string(), &v.to_string()]);
                let f = study(
                    root,
                    &dir,
                    &su,
                    &format!("2-{name}-{s}-{v}"),
                    2,
                    px,
                    |f, _| f.patient_id = id,
                );
                seeds.push((name, f));
                studies.push(StudyWant {
                    dir,
                    date: Some("20220115"),
                    source: "study_date",
                    person: person.clone(),
                });
            }
        }
        scenarios.push(Scenario {
            name,
            what,
            people: 4,
            needs: "the first path segment, because the tag says nothing",
            studies,
        });
    }

    // The code is in PatientName beside a date, so a pattern is the only way
    // to read it without taking the date too.
    {
        let name = "id-in-name";
        let mut studies = Vec::new();
        for s in 1..=3u64 {
            let person = format!("P{s}");
            let pn: &'static str = Box::leak(format!("NAME{s:03}^20220115").into_boxed_str());
            // Two visits each, so the study-UID fallback cannot give the right
            // count by accident.
            for v in 1..=2u64 {
                let dir = format!("{name}/anon{s}/visit{v}");
                let su = uid(&["3", &s.to_string(), &v.to_string()]);
                let f = study(root, &dir, &su, &format!("3-{s}-{v}"), 2, px, |f, _| {
                    f.patient_id = None;
                    f.patient_name = pn;
                });
                seeds.push((name, f));
                studies.push(StudyWant {
                    dir,
                    date: Some("20220115"),
                    source: "study_date",
                    person: person.clone(),
                });
            }
        }
        scenarios.push(Scenario {
            name,
            what: "the code hides in PatientName next to a date",
            people: 3,
            needs: "a pattern with an id group, so the date is not taken as identity",
            studies,
        });
    }

    // Two files of one study disagree, which no rule can silently resolve.
    {
        let name = "id-inconsistent";
        let dir = format!("{name}/mixed/visit1");
        let su = uid(&["4", "1"]);
        let f = study(root, &dir, &su, "4-1", 4, px, |f, i| {
            f.patient_id = Some(if i % 2 == 0 { "MIX001" } else { "MIX002" });
        });
        seeds.push((name, f));
        scenarios.push(Scenario {
            name,
            what: "one study whose files carry two different PatientIDs",
            // A study belongs to one person, so one is the only honest answer;
            // what the scenario checks is that the reader says the tree is
            // wrong rather than choosing quietly.
            people: 1,
            needs: "a diagnostic; the tree is wrong and the reader must say so",
            studies: vec![StudyWant {
                dir,
                date: Some("20220115"),
                source: "study_date",
                person: "ambiguous".into(),
            }],
        });
    }

    // The code is one level deeper than the usual layout.
    {
        let name = "id-depth2";
        let mut studies = Vec::new();
        for s in 1..=3u64 {
            let person = format!("P{s}");
            let dir = format!("{name}/site-a/DEEP{s:03}/visit1");
            let su = uid(&["5", &s.to_string()]);
            let f = study(root, &dir, &su, &format!("5-{s}"), 2, px, |f, _| {
                f.patient_id = Some("XXXX")
            });
            seeds.push((name, f));
            studies.push(StudyWant {
                dir,
                date: Some("20220115"),
                source: "study_date",
                person,
            });
        }
        scenarios.push(Scenario {
            name,
            what: "a site directory sits above the subject directory",
            people: 3,
            needs: "the second path segment, so the segment is a setting and not a constant",
            studies,
        });
    }

    // Names a filesystem allows and a naive reader mangles.
    {
        let name = "id-awkward-names";
        let folders = ["ÅÄÖ 001", "sub 002", "s+003", "s.004"];
        let mut studies = Vec::new();
        for (s, folder) in folders.iter().enumerate() {
            let person = format!("P{}", s + 1);
            let dir = format!("{name}/{folder}/visit1");
            let su = uid(&["6", &s.to_string()]);
            let f = study(root, &dir, &su, &format!("6-{s}"), 2, px, |f, _| {
                f.patient_id = Some("XXXX")
            });
            seeds.push((name, f));
            studies.push(StudyWant {
                dir,
                date: Some("20220115"),
                source: "study_date",
                person,
            });
        }
        scenarios.push(Scenario {
            name,
            what: "folder names with non-ASCII, a space, a plus and a dot",
            people: 4,
            needs: "the segment taken whole, and a code that survives the round trip",
            studies,
        });
    }

    // Two folders that differ only in case, which one filesystem separates and
    // another folds together.
    {
        let name = "id-case";
        let mut studies = Vec::new();
        for (s, folder) in ["CASE001", "case001"].iter().enumerate() {
            let dir = format!("{name}/{folder}/visit1");
            let su = uid(&["7", &s.to_string()]);
            let f = study(root, &dir, &su, &format!("7-{s}"), 2, px, |f, _| {
                f.patient_id = Some("XXXX")
            });
            seeds.push((name, f));
            studies.push(StudyWant {
                dir,
                date: Some("20220115"),
                source: "study_date",
                person: format!("P{}", s + 1),
            });
        }
        scenarios.push(Scenario {
            name,
            what: "two subject folders differing only in case",
            people: 2,
            needs: "two subjects on a case-sensitive filesystem, and a diagnostic either way",
            studies,
        });
    }

    // One person whose studies were filed under two different branches.
    {
        let name = "id-two-paths";
        let mut studies = Vec::new();
        for (v, branch) in ["branch-a", "branch-b"].iter().enumerate() {
            let dir = format!("{name}/{branch}/SAME001/visit{}", v + 1);
            let su = uid(&["8", &v.to_string()]);
            let f = study(root, &dir, &su, &format!("8-{v}"), 2, px, |f, _| {
                f.patient_id = Some("XXXX")
            });
            seeds.push((name, f));
            studies.push(StudyWant {
                dir,
                date: Some("20220115"),
                source: "study_date",
                person: "P1".into(),
            });
        }
        scenarios.push(Scenario {
            name,
            what: "one person's studies filed under two different branches",
            people: 1,
            needs: "the segment that holds the code, not the branch above it",
            studies,
        });
    }

    // ------------------------------------------------------------------ dates

    // The fallback chain, one rung at a time.
    struct DateCase {
        name: &'static str,
        what: &'static str,
        study: Option<&'static str>,
        series: Option<&'static str>,
        acquisition: Option<&'static str>,
        content: Option<&'static str>,
        instance_creation: Option<&'static str>,
        pps_start: Option<&'static str>,
        /// A date carried inside a Siemens CSA private element's version.
        csa: Option<&'static str>,
        /// Extra path components spliced into the study UID.
        uid_date: Option<&'static str>,
        series_uid_date: Option<&'static str>,
        /// Unix epoch seconds spliced into the SOP UID, which is how some GE
        /// scanners leave a timestamp behind.
        uid_epoch: Option<&'static str>,
        /// A `YYYYMMDD` component in the directory path itself.
        path_date: Option<&'static str>,
        want: Option<&'static str>,
        source: &'static str,
        needs: &'static str,
    }

    let dates = [
        DateCase {
            name: "date-good",
            what: "StudyDate present",
            study: Some("20220115"),
            series: Some("20220115"),
            acquisition: Some("20220115"),
            content: Some("20220115"),
            instance_creation: None,
            pps_start: None,
            csa: None,
            uid_date: None,
            series_uid_date: None,
            uid_epoch: None,
            path_date: None,
            want: Some("20220115"),
            source: "study_date",
            needs: "nothing",
        },
        DateCase {
            name: "date-series-only",
            what: "no StudyDate, SeriesDate present",
            study: None,
            series: Some("20220216"),
            acquisition: None,
            content: None,
            instance_creation: None,
            pps_start: None,
            csa: None,
            uid_date: None,
            series_uid_date: None,
            uid_epoch: None,
            path_date: None,
            want: Some("20220216"),
            source: "series_date",
            needs: "the first rung of the fallback",
        },
        DateCase {
            name: "date-acq-only",
            what: "only AcquisitionDate",
            study: None,
            series: None,
            acquisition: Some("20220317"),
            content: None,
            instance_creation: None,
            pps_start: None,
            csa: None,
            uid_date: None,
            series_uid_date: None,
            uid_epoch: None,
            path_date: None,
            want: Some("20220317"),
            source: "acquisition_date",
            needs: "the second rung",
        },
        DateCase {
            name: "date-content-only",
            what: "only ContentDate",
            study: None,
            series: None,
            acquisition: None,
            content: Some("20220418"),
            instance_creation: None,
            pps_start: None,
            csa: None,
            uid_date: None,
            series_uid_date: None,
            uid_epoch: None,
            path_date: None,
            want: Some("20220418"),
            source: "content_date",
            needs: "the third rung",
        },
        DateCase {
            name: "date-uid-only",
            what: "no date field anywhere; the study UID carries one",
            study: None,
            series: None,
            acquisition: None,
            content: None,
            instance_creation: None,
            pps_start: None,
            csa: None,
            uid_date: Some("20220519"),
            series_uid_date: None,
            uid_epoch: None,
            path_date: None,
            want: Some("20220519"),
            source: "uid",
            needs: "reading eight digits out of a UID, and believing them",
        },
        DateCase {
            name: "date-uid-series",
            what: "no date field; only the series UID carries one",
            study: None,
            series: None,
            acquisition: None,
            content: None,
            instance_creation: None,
            pps_start: None,
            csa: None,
            uid_date: None,
            series_uid_date: Some("20220620"),
            uid_epoch: None,
            path_date: None,
            want: Some("20220620"),
            source: "uid",
            needs: "trying more than the study UID",
        },
        DateCase {
            name: "date-none",
            what: "no date field and no date-shaped digits in any UID",
            study: None,
            series: None,
            acquisition: None,
            content: None,
            instance_creation: None,
            pps_start: None,
            csa: None,
            uid_date: None,
            series_uid_date: None,
            uid_epoch: None,
            path_date: None,
            want: None,
            source: "none",
            needs: "a review item; this study cannot be placed in a session",
        },
        DateCase {
            name: "date-uid-trap-invalid",
            what: "the UID holds 20221345, which is not a calendar date",
            study: None,
            series: None,
            acquisition: None,
            content: None,
            instance_creation: None,
            pps_start: None,
            csa: None,
            uid_date: Some("20221345"),
            series_uid_date: None,
            uid_epoch: None,
            path_date: None,
            want: None,
            source: "none",
            needs: "refusing eight digits that do not parse as a date",
        },
        DateCase {
            name: "date-uid-trap-range",
            what: "the UID holds 17000101, a real date far outside the range",
            study: None,
            series: None,
            acquisition: None,
            content: None,
            instance_creation: None,
            pps_start: None,
            csa: None,
            uid_date: Some("17000101"),
            series_uid_date: None,
            uid_epoch: None,
            path_date: None,
            want: None,
            source: "none",
            needs: "the year range, which is the only thing making a UID date reasonable",
        },
        DateCase {
            name: "date-implausible",
            what: "StudyDate is 18990101, present and absurd",
            study: Some("18990101"),
            series: None,
            acquisition: None,
            content: None,
            instance_creation: None,
            pps_start: None,
            csa: None,
            uid_date: None,
            series_uid_date: None,
            uid_epoch: None,
            path_date: None,
            want: Some("18990101"),
            source: "study_date",
            needs: "keeping what was measured, and raising a diagnostic about it",
        },
        DateCase {
            name: "date-malformed",
            what: "StudyDate is 2022-01-15, non-conformant but unambiguous",
            study: Some("2022-01-15"),
            series: Some("20220721"),
            acquisition: None,
            content: None,
            instance_creation: None,
            pps_start: None,
            csa: None,
            uid_date: None,
            series_uid_date: None,
            uid_epoch: None,
            path_date: None,
            want: Some("20220115"),
            source: "study_date",
            needs: "reading a non-conformant but unambiguous value rather than refusing it",
        },
        DateCase {
            name: "date-zero",
            what: "StudyDate is 00000000",
            study: Some("00000000"),
            series: Some("20220722"),
            acquisition: None,
            content: None,
            instance_creation: None,
            pps_start: None,
            csa: None,
            uid_date: None,
            series_uid_date: None,
            uid_epoch: None,
            path_date: None,
            want: Some("20220722"),
            source: "series_date",
            needs: "treating the zero a scanner writes instead of nothing as no value, rather than storing it as a date",
        },
        DateCase {
            name: "date-disagree",
            what: "the four date fields hold four different days",
            study: Some("20220801"),
            series: Some("20220802"),
            acquisition: Some("20220803"),
            content: Some("20220804"),
            instance_creation: None,
            pps_start: None,
            csa: None,
            uid_date: None,
            series_uid_date: None,
            uid_epoch: None,
            path_date: None,
            want: Some("20220801"),
            source: "study_date",
            needs: "the order of preference, and a diagnostic that they disagree",
        },
        DateCase {
            name: "date-instance-creation",
            what: "only InstanceCreationDate survived the scrub",
            study: None,
            series: None,
            acquisition: None,
            content: None,
            instance_creation: Some("20220901"),
            pps_start: None,
            csa: None,
            uid_date: None,
            series_uid_date: None,
            uid_epoch: None,
            path_date: None,
            want: Some("20220901"),
            source: "instance_creation_date",
            needs: "looking past the four obvious date elements",
        },
        DateCase {
            name: "date-pps",
            what: "only the performed procedure step start date survived",
            study: None,
            series: None,
            acquisition: None,
            content: None,
            instance_creation: None,
            pps_start: Some("20221003"),
            csa: None,
            uid_date: None,
            series_uid_date: None,
            uid_epoch: None,
            path_date: None,
            want: Some("20221003"),
            source: "pps_start_date",
            needs: "the procedure step, which some scrubs leave alone",
        },
        DateCase {
            name: "date-private-csa",
            what: "the only date left is inside a Siemens CSA private element",
            study: None,
            series: None,
            acquisition: None,
            content: None,
            instance_creation: None,
            pps_start: None,
            csa: Some("20221104"),
            uid_date: None,
            series_uid_date: None,
            uid_epoch: None,
            path_date: None,
            want: Some("20221104"),
            source: "private",
            needs: "resolving a private creator and reading a date out of a version string",
        },
        DateCase {
            name: "date-jan-first-trap",
            what: "InstanceCreationDate is a first of January and SeriesDate is not",
            study: None,
            series: Some("20220615"),
            acquisition: None,
            content: None,
            instance_creation: Some("20220101"),
            pps_start: None,
            csa: None,
            uid_date: None,
            series_uid_date: None,
            uid_epoch: None,
            path_date: None,
            want: Some("20220615"),
            source: "series_date",
            needs: "distrusting a first of January when any other candidate exists",
        },
        DateCase {
            name: "date-1900",
            what: "StudyDate is 19000101, an anonymiser's idea of nothing",
            study: Some("19000101"),
            series: Some("20221205"),
            acquisition: None,
            content: None,
            instance_creation: None,
            pps_start: None,
            csa: None,
            uid_date: None,
            series_uid_date: None,
            uid_epoch: None,
            path_date: None,
            want: Some("20221205"),
            source: "series_date",
            needs: "a placeholder list, not just a parse",
        },
        DateCase {
            name: "date-uid-epoch",
            what: "no date anywhere; the SOP UID carries Unix epoch seconds",
            study: None,
            series: None,
            acquisition: None,
            content: None,
            instance_creation: None,
            pps_start: None,
            csa: None,
            uid_date: None,
            series_uid_date: None,
            uid_epoch: Some("1572249167"),
            path_date: None,
            want: Some("20191028"),
            source: "uid_epoch",
            needs: "reading a timestamp, not only a YYYYMMDD, out of a UID",
        },
        DateCase {
            name: "date-in-path",
            what: "no date in any element; the directory name is the date",
            study: None,
            series: None,
            acquisition: None,
            content: None,
            instance_creation: None,
            pps_start: None,
            csa: None,
            uid_date: None,
            series_uid_date: None,
            uid_epoch: None,
            path_date: Some("20220815"),
            want: Some("20220815"),
            source: "path",
            needs: "the path as a source, which is where a sorted archive puts it",
        },
        DateCase {
            name: "date-outvoted",
            what: "two independent sources agree and one disagrees",
            study: None,
            series: Some("20230301"),
            acquisition: Some("20230301"),
            content: Some("20230415"),
            instance_creation: None,
            pps_start: None,
            csa: None,
            uid_date: None,
            series_uid_date: None,
            uid_epoch: None,
            path_date: None,
            want: Some("20230301"),
            source: "vote",
            needs: "weighing sources rather than taking the first that answers",
        },
    ];

    for (n, c) in dates.iter().enumerate() {
        // A sorted archive puts the date in the path, so one scenario does too.
        let dir = match c.path_date {
            Some(d) => format!("{}/DATE{:03}/{d}/visit1", c.name, n + 1),
            None => format!("{}/DATE{:03}/visit1", c.name, n + 1),
        };
        let su = match c.uid_date {
            Some(d) => uid(&["10", &n.to_string(), d, "1"]),
            None => uid(&["10", &n.to_string(), "1"]),
        };
        let seru = match c.series_uid_date {
            Some(d) => uid(&["10", &n.to_string(), d, "s1"]),
            None => uid(&["10", &n.to_string(), "s1"]),
        };
        let series_uid = seru.clone();
        let epoch = c.uid_epoch;
        let f = study(root, &dir, &su, &format!("10-{n}"), 2, px, move |f, i| {
            f.patient_id = Some("DATECASE");
            f.series_uid = series_uid.clone();
            f.study_date = c.study;
            f.series_date = c.series;
            f.acquisition_date = c.acquisition;
            f.content_date = c.content;
            f.instance_creation_date = c.instance_creation;
            f.pps_start_date = c.pps_start;
            f.csa_version = c.csa;
            if let Some(ts) = epoch {
                // GE leaves the timestamp in the SOP UID rather than in a date
                // element, so the UID is where a reader has to look.
                f.sop = uid(&["10", "ge", ts, &i.to_string()]);
            }
        });
        seeds.push((c.name, f));
        scenarios.push(Scenario {
            name: c.name,
            what: c.what,
            people: 1,
            needs: c.needs,
            studies: vec![StudyWant {
                dir,
                date: c.want,
                source: c.source,
                person: "P1".into(),
            }],
        });
    }

    // --------------------------------------------------------------- sessions

    struct SessionCase {
        name: &'static str,
        what: &'static str,
        needs: &'static str,
        days: &'static [&'static str],
    }

    let sessions = [
        SessionCase {
            name: "ses-same-day",
            what: "two studies on one day",
            needs: "one session, whatever the window",
            days: &["20220901", "20220901"],
        },
        SessionCase {
            name: "ses-window",
            what: "two studies three days apart",
            needs: "two sessions at window 0, one at window 14",
            days: &["20221001", "20221004"],
        },
        SessionCase {
            name: "ses-cadence",
            what: "visits at zero, six, nine and twelve months",
            needs: "M00, M06, M09 and M12: the ninth keeps its real month",
            days: &["20220101", "20220703", "20221002", "20230101"],
        },
        SessionCase {
            name: "ses-pre-anchor",
            what: "a visit six months before the anchor",
            needs: "PRE06 and M00, never M-06, because a hyphen is BIDS's separator",
            days: &["20211201", "20220601"],
        },
    ];

    for (n, c) in sessions.iter().enumerate() {
        let mut studies = Vec::new();
        for (v, day) in c.days.iter().enumerate() {
            let dir = format!("{}/SES{:03}/visit{}", c.name, n + 1, v + 1);
            let su = uid(&["11", &n.to_string(), &v.to_string()]);
            let d: &'static str = day;
            let f = study(
                root,
                &dir,
                &su,
                &format!("11-{n}-{v}"),
                2,
                px,
                move |f, _| {
                    f.patient_id = Some("SESCASE");
                    f.study_date = Some(d);
                    f.series_date = Some(d);
                    f.acquisition_date = Some(d);
                    f.content_date = Some(d);
                },
            );
            seeds.push((c.name, f));
            studies.push(StudyWant {
                dir,
                date: Some(d),
                source: "study_date",
                person: "P1".into(),
            });
        }
        scenarios.push(Scenario {
            name: c.name,
            what: c.what,
            people: 1,
            needs: c.needs,
            studies,
        });
    }

    // ----------------------------------------------------------------- the mess

    // A scenario writes one seed per study; the first is its representative,
    // and the mess is built once from it, so it belongs to that scenario and
    // adds no subject and no study the scenario did not declare.
    let mut done: Vec<&str> = Vec::new();
    for (name, seed) in &seeds {
        if done.contains(name) {
            continue;
        }
        done.push(name);
        sprinkle(root, &mut rng, name, seed, px);
    }

    // ------------------------------------------------------------- the manifest

    let mut out = std::io::stdout().lock();
    writeln!(out, "{{").unwrap();
    writeln!(out, "  \"seed\": {},", o.seed).unwrap();
    writeln!(out, "  \"scenarios\": [").unwrap();
    for (i, s) in scenarios.iter().enumerate() {
        writeln!(out, "    {{").unwrap();
        writeln!(out, "      \"name\": {:?},", s.name).unwrap();
        writeln!(out, "      \"what\": {:?},", s.what).unwrap();
        writeln!(out, "      \"needs\": {:?},", s.needs).unwrap();
        writeln!(out, "      \"people\": {},", s.people).unwrap();
        writeln!(out, "      \"rule\": {:?},", rule_of(s.name)).unwrap();
        match diagnostic_of(s.name) {
            Some(d) => writeln!(out, "      \"diagnostic\": {d:?},").unwrap(),
            None => writeln!(out, "      \"diagnostic\": null,").unwrap(),
        }
        writeln!(out, "      \"studies\": [").unwrap();
        for (j, w) in s.studies.iter().enumerate() {
            let date = match w.date {
                Some(d) => format!("{d:?}"),
                None => "null".into(),
            };
            writeln!(
                out,
                "        {{\"dir\": {:?}, \"person\": {:?}, \"date\": {}, \"source\": {:?}}}{}",
                w.dir,
                w.person,
                date,
                w.source,
                if j + 1 == s.studies.len() { "" } else { "," }
            )
            .unwrap();
        }
        writeln!(out, "      ]").unwrap();
        writeln!(
            out,
            "    }}{}",
            if i + 1 == scenarios.len() { "" } else { "," }
        )
        .unwrap();
    }
    writeln!(out, "  ]").unwrap();
    writeln!(out, "}}").unwrap();

    let studies: usize = scenarios.iter().map(|s| s.studies.len()).sum();
    eprintln!(
        "awkward: {} scenarios, {} studies, {} people, under {}",
        scenarios.len(),
        studies,
        scenarios.iter().map(|s| s.people).sum::<usize>(),
        root.display()
    );
}
