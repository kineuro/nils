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
    /// What `nils session` must derive from this subtree, per scheme. Empty
    /// for a scenario that is about identity or dates rather than timelines.
    sessions: Vec<SessionWant>,
    /// What the fingerprint must work out for this subtree's one stack (§6).
    fingerprint: Option<FingerprintWant>,
}

/// The derived columns of one stack, as the manifest states them. `None` is a
/// column the reader must leave empty, which is as much of the answer as a
/// value is: v0 fills every one of these whatever the input, and half this
/// wave's argument is that a repair that always answers is not a repair.
struct FingerprintWant {
    field_strength_tesla: Option<&'static str>,
    field_strength_normalized: Option<&'static str>,
    field_strength_unit: Option<&'static str>,
    acquisition_type_filled: Option<&'static str>,
    acquisition_type_source: Option<&'static str>,
    image_role: &'static str,
    dwi_b_value: Option<&'static str>,
    dwi_b_values: Option<&'static str>,
    dwi_b_value_source: Option<&'static str>,
    dwi_pe_direction: Option<&'static str>,
    dwi_pe_direction_source: Option<&'static str>,
    dwi_directions: Option<&'static str>,
    dwi_directions_source: Option<&'static str>,
}

/// One scheme, and the labels it must produce for the scenario's one subject.
struct SessionWant {
    /// Why this scheme is the interesting one here.
    what: &'static str,
    /// The scheme's YAML, written into the run directory by the gate.
    scheme: &'static str,
    /// The labels, in date order. `null` is a session the scheme left
    /// unlabelled, which the caller renders as its date.
    labels: Vec<&'static str>,
    /// How many of them the scheme must flag as worth a look.
    flagged: usize,
    /// Whether each session holds a stack the scanner called its output:
    /// `yes`, `no` or `unknown`, in the same order as the labels. Empty when
    /// the scenario is not about the rescue. `no` is its condition (§6).
    primaries: Vec<&'static str>,
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
    /// `SeriesDescription`, which is most of what the text tier reads.
    series_description: &'static str,
    /// `SequenceName`, where Siemens puts the dimensionality.
    sequence_name: Option<&'static str>,
    /// `MRAcquisitionType`. `None` writes no element, which is what a scanner
    /// that never filled it in leaves behind and what the fill exists for.
    mr_acquisition_type: Option<&'static str>,
    /// `ImageType`, whose first two values say what the stack is.
    image_type: &'static str,
    /// `MagneticFieldStrength`, in whatever unit the scanner felt like.
    field_strength: Option<&'static str>,
    /// The diffusion values, which differ from one image of a series to the
    /// next: that is what a multi-shell, multi-direction acquisition is.
    b_value: Option<String>,
    gradient: Option<String>,
    directionality: Option<&'static str>,
    /// The Siemens private b value, under its own creator block.
    siemens_b_value: Option<String>,
    /// `ImageOrientationPatient` and `InPlanePhaseEncodingDirection`, which
    /// with the CSA flag below are what a phase direction is computed from.
    iop: Option<&'static str>,
    in_plane: Option<&'static str>,
    /// A real SV10 CSA header carrying `PhaseEncodingDirectionPositive`.
    pe_positive: Option<&'static str>,
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
            series_description: "ax t1 mprage",
            sequence_name: None,
            mr_acquisition_type: Some("3D"),
            image_type: "ORIGINAL\\PRIMARY\\M\\ND",
            field_strength: Some("3.0"),
            b_value: None,
            gradient: None,
            directionality: None,
            siemens_b_value: None,
            iop: None,
            in_plane: None,
            pe_positive: None,
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
        f.series_description,
    ));
    e.push(synth::text(tags::SCANNING_SEQUENCE, VR::CS, "GR"));
    e.push(synth::text(tags::SEQUENCE_VARIANT, VR::CS, "SK\\SP\\MP"));
    if let Some(v) = f.sequence_name {
        e.push(synth::text(tags::SEQUENCE_NAME, VR::SH, v));
    }
    if let Some(v) = f.mr_acquisition_type {
        e.push(synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, v));
    }
    if let Some(v) = f.field_strength {
        e.push(synth::text(tags::MAGNETIC_FIELD_STRENGTH, VR::DS, v));
    }
    e.push(synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"));
    e.push(synth::text(tags::IMAGE_TYPE, VR::CS, f.image_type));
    if let Some(v) = &f.b_value {
        e.push(fd(tags::DIFFUSION_B_VALUE, v));
    }
    if let Some(v) = &f.gradient {
        e.push(fd(tags::DIFFUSION_GRADIENT_ORIENTATION, v));
    }
    if let Some(v) = f.directionality {
        e.push(synth::text(tags::DIFFUSION_DIRECTIONALITY, VR::CS, v));
    }
    if let Some(v) = &f.siemens_b_value {
        // The creator claims the block, so the value sits at (0019,10xx) and a
        // reader that looks up the creator finds it wherever the block landed.
        e.push(synth::text(
            dicom_core::Tag(0x0019, 0x0010),
            VR::LO,
            "SIEMENS MR HEADER",
        ));
        e.push(synth::text(dicom_core::Tag(0x0019, 0x100C), VR::IS, v));
    }
    if let Some(v) = f.iop {
        e.push(synth::text(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, v));
    }
    if let Some(v) = f.in_plane {
        e.push(synth::text(
            tags::IN_PLANE_PHASE_ENCODING_DIRECTION,
            VR::CS,
            v,
        ));
    }
    if let Some(v) = f.pe_positive {
        e.push(synth::text(
            dicom_core::Tag(0x0029, 0x0010),
            VR::LO,
            "SIEMENS CSA HEADER",
        ));
        e.push(synth::bytes(
            dicom_core::Tag(0x0029, 0x1010),
            VR::OB,
            nils_dicom::csa::build_sv10(&[("PhaseEncodingDirectionPositive", &[v])]),
        ));
    }
    e.push(synth::us(tags::ROWS, 64));
    e.push(synth::us(tags::COLUMNS, 64));
    e.push(synth::bytes(
        tags::PIXEL_DATA,
        VR::OW,
        vec![0u8; pixel_bytes],
    ));
    e
}

/// An FD element from the backslash-separated way a value is written down.
/// The diffusion values are binary doubles on disk and text in the catalogue,
/// so the corpus writes what a scanner writes and the reader does the joining.
fn fd(tag: dicom_core::Tag, values: &str) -> Elem {
    let bytes: Vec<u8> = values
        .split('\\')
        .filter_map(|p| p.trim().parse::<f64>().ok())
        .flat_map(f64::to_le_bytes)
        .collect();
    synth::bytes(tag, VR::FD, bytes)
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
            sessions: Vec::new(),
            fingerprint: None,
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
            sessions: Vec::new(),
            fingerprint: None,
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
            sessions: Vec::new(),
            fingerprint: None,
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
            sessions: Vec::new(),
            fingerprint: None,
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
            sessions: Vec::new(),
            fingerprint: None,
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
            sessions: Vec::new(),
            fingerprint: None,
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
            sessions: Vec::new(),
            fingerprint: None,
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
            sessions: Vec::new(),
            fingerprint: None,
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
            sessions: Vec::new(),
            fingerprint: None,
        });
    }

    // --------------------------------------------------------------- sessions

    struct SessionCase {
        name: &'static str,
        what: &'static str,
        needs: &'static str,
        days: &'static [&'static str],
        /// The visit directory each study sits in. An archive that named its
        /// folders is evidence, so a scenario about that names them; the rest
        /// use `visit1`, `visit2`.
        folders: &'static [&'static str],
        /// What `nils session` must derive, per scheme.
        checks: &'static [SessionCheck],
    }

    struct SessionCheck {
        what: &'static str,
        scheme: &'static str,
        labels: &'static [&'static str],
        flagged: usize,
        primaries: &'static [&'static str],
    }

    const CADENCE: &str = "session:\n  naming:\n    months:\n      cadence: [0, 6, 12, 18, 24]\n      tolerance: 1.0\n";

    let sessions = [
        SessionCase {
            name: "ses-same-day",
            what: "two studies on one day",
            needs: "one session, whatever the window",
            days: &["20220901", "20220901"],
            folders: &["visit1", "visit2"],
            checks: &[SessionCheck {
                what: "a day is a visit, and the default window is a day",
                scheme: "session: {}\n",
                labels: &["20220901"],
                flagged: 0,
                primaries: &[],
            }],
        },
        SessionCase {
            name: "ses-window",
            what: "two studies three days apart",
            needs: "two sessions at window 0, one at window 14",
            days: &["20221001", "20221004"],
            folders: &["visit1", "visit2"],
            checks: &[
                SessionCheck {
                    what: "v0 keys on the day, so a split appointment is two visits",
                    scheme: "session:\n  window_days: 0\n",
                    labels: &["20221001", "20221004"],
                    flagged: 0,
                    primaries: &[],
                },
                SessionCheck {
                    what: "the seam v0 left: a brain study and a spine study are one visit",
                    scheme: "session:\n  window_days: 14\n",
                    labels: &["20221001"],
                    flagged: 0,
                    primaries: &[],
                },
            ],
        },
        SessionCase {
            name: "ses-cadence",
            what: "visits at zero, six, nine and twelve months",
            needs: "M00, M06, M09 and M12: the ninth keeps its real month",
            days: &["20220101", "20220703", "20221002", "20230101"],
            folders: &["visit1", "visit2", "visit3", "visit4"],
            checks: &[SessionCheck {
                what: "an off-schedule visit reads as when it happened, not as nothing",
                scheme: CADENCE,
                labels: &["M00", "M06", "M09", "M12"],
                flagged: 0,
                primaries: &[],
            }],
        },
        SessionCase {
            name: "ses-pre-anchor",
            what: "a workup scan six months before the anchor",
            needs: "PRE06 and M00, never M-06, because a hyphen is BIDS's separator",
            days: &["20211201", "20220601"],
            folders: &["PRE06", "M00"],
            checks: &[SessionCheck {
                what: "month zero read back out of the folders, and the earlier scan is PRE06",
                scheme: "session:\n  anchor: source_label\n  naming:\n    months:\n      cadence: [0, 6, 12]\n      tolerance: 1.0\n  said:\n    segment: 2\n",
                labels: &["PRE06", "M00"],
                flagged: 1,
                primaries: &[],
            }],
        },
        SessionCase {
            name: "ses-fragment",
            what: "an archive that starts at the six-month visit and says so",
            needs: "M06 and M12 from the folders, not M00 and M06 from the dates",
            days: &["20220701", "20230101"],
            folders: &["M06", "M12"],
            checks: &[
                SessionCheck {
                    what: "the dates alone call the earliest scan we hold the baseline",
                    scheme: CADENCE,
                    labels: &["M00", "M06"],
                    flagged: 0,
                    primaries: &[],
                },
                SessionCheck {
                    what: "and the folders say it is not, which is a finding",
                    scheme: "session:\n  naming:\n    months:\n      cadence: [0, 6, 12, 18, 24]\n      tolerance: 1.0\n  said:\n    segment: 2\n",
                    labels: &["M00", "M06"],
                    flagged: 2,
                    primaries: &[],
                },
                SessionCheck {
                    what: "believing the folders puts month zero where the archive says",
                    scheme: "session:\n  anchor: source_label\n  naming:\n    months:\n      cadence: [0, 6, 12, 18, 24]\n      tolerance: 1.0\n  said:\n    segment: 2\n",
                    labels: &["M06", "M12"],
                    flagged: 0,
                    primaries: &[],
                },
            ],
        },
    ];

    for (n, c) in sessions.iter().enumerate() {
        let mut studies = Vec::new();
        for (v, day) in c.days.iter().enumerate() {
            let dir = format!("{}/SES{:03}/{}", c.name, n + 1, c.folders[v]);
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
            sessions: c
                .checks
                .iter()
                .map(|k| SessionWant {
                    what: k.what,
                    scheme: k.scheme,
                    labels: k.labels.to_vec(),
                    flagged: k.flagged,
                    primaries: k.primaries.to_vec(),
                })
                .collect(),
            fingerprint: None,
        });
    }

    // ------------------------------------------------------ derived fields

    // §6. Each is one study of one stack, because these are pure functions of
    // one row: what is being checked is that the right column reaches the
    // right function and the answer lands in the right column, which is the
    // class of mistake a unit test cannot see.
    struct FpCase {
        name: &'static str,
        what: &'static str,
        needs: &'static str,
        /// What the scanner wrote.
        field: Option<&'static str>,
        acq: Option<&'static str>,
        seq: Option<&'static str>,
        image_type: &'static str,
        description: &'static str,
        want: FingerprintWant,
    }

    const PRIMARY: &str = "ORIGINAL\\PRIMARY\\M\\ND";

    let fps = [
        FpCase {
            name: "fp-field-gauss",
            what: "a field strength written in gauss",
            needs: "15000 G is 1.5 T, converted rather than rounded",
            field: Some("15000"),
            acq: Some("3D"),
            seq: None,
            image_type: PRIMARY,
            description: "ax t1 mprage",
            want: FingerprintWant {
                field_strength_tesla: Some("1.5"),
                field_strength_normalized: Some("1.5"),
                field_strength_unit: Some("gauss"),
                acquisition_type_filled: None,
                acquisition_type_source: None,
                image_role: "original_primary",
                dwi_b_value: None,
                dwi_b_values: None,
                dwi_b_value_source: None,
                dwi_pe_direction: None,
                dwi_pe_direction_source: None,
                dwi_directions: None,
                dwi_directions_source: None,
            },
        },
        FpCase {
            name: "fp-field-milli",
            what: "a field strength written in millitesla",
            needs: "1500 mT is 1.5 T; v0 divides by ten thousand and calls it 0.5 T",
            field: Some("1500"),
            acq: Some("3D"),
            seq: None,
            image_type: PRIMARY,
            description: "ax t1 mprage",
            want: FingerprintWant {
                field_strength_tesla: Some("1.5"),
                field_strength_normalized: Some("1.5"),
                field_strength_unit: Some("millitesla"),
                acquisition_type_filled: None,
                acquisition_type_source: None,
                image_role: "original_primary",
                dwi_b_value: None,
                dwi_b_values: None,
                dwi_b_value_source: None,
                dwi_pe_direction: None,
                dwi_pe_direction_source: None,
                dwi_directions: None,
                dwi_directions_source: None,
            },
        },
        FpCase {
            name: "fp-field-off-grid",
            what: "a 4.7 T animal scanner",
            needs: "no normalised value; v0 records it as 3 T and loses the reading",
            field: Some("4.7"),
            acq: Some("3D"),
            seq: None,
            image_type: PRIMARY,
            description: "ax t1 mprage",
            want: FingerprintWant {
                field_strength_tesla: Some("4.7"),
                field_strength_normalized: None,
                field_strength_unit: Some("tesla"),
                acquisition_type_filled: None,
                acquisition_type_source: None,
                image_role: "original_primary",
                dwi_b_value: None,
                dwi_b_values: None,
                dwi_b_value_source: None,
                dwi_pe_direction: None,
                dwi_pe_direction_source: None,
                dwi_directions: None,
                dwi_directions_source: None,
            },
        },
        FpCase {
            name: "fp-field-absent",
            what: "no field strength at all",
            needs: "nothing derived, and nothing invented",
            field: None,
            acq: Some("3D"),
            seq: None,
            image_type: PRIMARY,
            description: "ax t1 mprage",
            want: FingerprintWant {
                field_strength_tesla: None,
                field_strength_normalized: None,
                field_strength_unit: None,
                acquisition_type_filled: None,
                acquisition_type_source: None,
                image_role: "original_primary",
                dwi_b_value: None,
                dwi_b_values: None,
                dwi_b_value_source: None,
                dwi_pe_direction: None,
                dwi_pe_direction_source: None,
                dwi_directions: None,
                dwi_directions_source: None,
            },
        },
        FpCase {
            name: "fp-acq-image-type",
            what: "no MRAcquisitionType, but ImageType says DIS3D",
            needs: "3D from the token, which beats the 2D word in the description",
            field: Some("3.0"),
            acq: None,
            seq: Some("tse2d1_9"),
            image_type: "ORIGINAL\\PRIMARY\\M\\DIS3D",
            description: "ax t2 tse 2d",
            want: FingerprintWant {
                field_strength_tesla: Some("3.0"),
                field_strength_normalized: Some("3.0"),
                field_strength_unit: Some("tesla"),
                acquisition_type_filled: Some("3D"),
                acquisition_type_source: Some("image_type"),
                image_role: "original_primary",
                dwi_b_value: None,
                dwi_b_values: None,
                dwi_b_value_source: None,
                dwi_pe_direction: None,
                dwi_pe_direction_source: None,
                dwi_directions: None,
                dwi_directions_source: None,
            },
        },
        FpCase {
            name: "fp-acq-sequence",
            what: "no MRAcquisitionType, and the sequence name says spc",
            needs: "3D from the sequence name, which beats haste in the description",
            field: Some("3.0"),
            acq: None,
            seq: Some("*spc_314ns"),
            image_type: PRIMARY,
            description: "t2 haste cor",
            want: FingerprintWant {
                field_strength_tesla: Some("3.0"),
                field_strength_normalized: Some("3.0"),
                field_strength_unit: Some("tesla"),
                acquisition_type_filled: Some("3D"),
                acquisition_type_source: Some("sequence_name"),
                image_role: "original_primary",
                dwi_b_value: None,
                dwi_b_values: None,
                dwi_b_value_source: None,
                dwi_pe_direction: None,
                dwi_pe_direction_source: None,
                dwi_directions: None,
                dwi_directions_source: None,
            },
        },
        FpCase {
            name: "fp-acq-text",
            what: "no MRAcquisitionType and nothing but the description",
            needs: "2D from the word haste, and the source says it was the text",
            field: Some("3.0"),
            acq: None,
            seq: None,
            image_type: PRIMARY,
            description: "t2 haste cor",
            want: FingerprintWant {
                field_strength_tesla: Some("3.0"),
                field_strength_normalized: Some("3.0"),
                field_strength_unit: Some("tesla"),
                acquisition_type_filled: Some("2D"),
                acquisition_type_source: Some("text"),
                image_role: "original_primary",
                dwi_b_value: None,
                dwi_b_values: None,
                dwi_b_value_source: None,
                dwi_pe_direction: None,
                dwi_pe_direction_source: None,
                dwi_directions: None,
                dwi_directions_source: None,
            },
        },
        FpCase {
            name: "fp-acq-unknowable",
            what: "no MRAcquisitionType and nothing anywhere that says 2D or 3D",
            needs: "no fill: v0's last tier reads the technique the classifier assigned",
            field: Some("3.0"),
            acq: None,
            seq: Some("tfl"),
            image_type: PRIMARY,
            description: "localizer",
            want: FingerprintWant {
                field_strength_tesla: Some("3.0"),
                field_strength_normalized: Some("3.0"),
                field_strength_unit: Some("tesla"),
                acquisition_type_filled: None,
                acquisition_type_source: None,
                image_role: "original_primary",
                dwi_b_value: None,
                dwi_b_values: None,
                dwi_b_value_source: None,
                dwi_pe_direction: None,
                dwi_pe_direction_source: None,
                dwi_directions: None,
                dwi_directions_source: None,
            },
        },
        FpCase {
            name: "fp-role-secondary",
            what: "an original image the scanner never called primary",
            needs: "original_secondary, which is what a session rescue looks for",
            field: Some("1.5"),
            acq: Some("2D"),
            seq: None,
            image_type: "ORIGINAL\\SECONDARY\\M\\ND",
            description: "ax t2 tse",
            want: FingerprintWant {
                field_strength_tesla: Some("1.5"),
                field_strength_normalized: Some("1.5"),
                field_strength_unit: Some("tesla"),
                acquisition_type_filled: None,
                acquisition_type_source: None,
                image_role: "original_secondary",
                dwi_b_value: None,
                dwi_b_values: None,
                dwi_b_value_source: None,
                dwi_pe_direction: None,
                dwi_pe_direction_source: None,
                dwi_directions: None,
                dwi_directions_source: None,
            },
        },
        FpCase {
            name: "fp-role-screenshot",
            what: "a screen capture labelled ORIGINAL and SECONDARY",
            needs: "not_an_image, so a rescue never picks it up",
            field: Some("1.5"),
            acq: Some("2D"),
            seq: None,
            image_type: "ORIGINAL\\SECONDARY\\SCREENSHOT",
            description: "patient protocol",
            want: FingerprintWant {
                field_strength_tesla: Some("1.5"),
                field_strength_normalized: Some("1.5"),
                field_strength_unit: Some("tesla"),
                acquisition_type_filled: None,
                acquisition_type_source: None,
                image_role: "not_an_image",
                dwi_b_value: None,
                dwi_b_values: None,
                dwi_b_value_source: None,
                dwi_pe_direction: None,
                dwi_pe_direction_source: None,
                dwi_directions: None,
                dwi_directions_source: None,
            },
        },
        FpCase {
            name: "fp-role-derived",
            what: "a workstation reformat",
            needs: "derived, whatever else the ImageType carries",
            field: Some("1.5"),
            acq: Some("2D"),
            seq: None,
            image_type: "DERIVED\\SECONDARY\\MPR",
            description: "cor mpr",
            want: FingerprintWant {
                field_strength_tesla: Some("1.5"),
                field_strength_normalized: Some("1.5"),
                field_strength_unit: Some("tesla"),
                acquisition_type_filled: None,
                acquisition_type_source: None,
                image_role: "derived",
                dwi_b_value: None,
                dwi_b_values: None,
                dwi_b_value_source: None,
                dwi_pe_direction: None,
                dwi_pe_direction_source: None,
                dwi_directions: None,
                dwi_directions_source: None,
            },
        },
    ];

    for (n, c) in fps.iter().enumerate() {
        let dir = format!("{}/FP{:03}/visit1", c.name, n + 1);
        let su = uid(&["12", &n.to_string()]);
        let f = study(root, &dir, &su, &format!("12-{n}"), 2, px, |f, _| {
            f.patient_id = Some("FPCASE");
            f.field_strength = c.field;
            f.mr_acquisition_type = c.acq;
            f.sequence_name = c.seq;
            f.image_type = c.image_type;
            f.series_description = c.description;
        });
        seeds.push((c.name, f));
        scenarios.push(Scenario {
            name: c.name,
            what: c.what,
            people: 1,
            needs: c.needs,
            studies: vec![StudyWant {
                dir,
                date: Some("20220115"),
                source: "study_date",
                person: "P1".into(),
            }],
            sessions: Vec::new(),
            fingerprint: Some(FingerprintWant { ..c.want }),
        });
    }

    // ------------------------------------------------------- the session rescue

    // §6. Some exports tag every reconstruction ORIGINAL\SECONDARY and never
    // write a primary at all, so the ordinary exclusion of
    // secondary-without-primary throws the whole visit away and the subject
    // becomes unusable. Whether that has happened is a question about the
    // occasion, not about the stack, which is why it is derived and not stored.
    struct ResCase {
        name: &'static str,
        what: &'static str,
        needs: &'static str,
        /// One study each: the day, and what its ImageType says.
        studies: &'static [(&'static str, &'static str)],
        checks: &'static [SessionCheck],
        fingerprint: Option<FingerprintWant>,
    }

    const SECONDARY: &str = "ORIGINAL\\SECONDARY\\M\\ND";
    const SAME_DAY: &str = "session: {}\n";
    const A_FORTNIGHT: &str = "session:\n  window_days: 14\n";

    let rescues = [
        ResCase {
            name: "res-no-primary",
            what: "a visit whose every stack is ORIGINAL and SECONDARY",
            needs: "the session says it holds no primary, which is the rescue's condition",
            studies: &[("20220901", SECONDARY), ("20220901", SECONDARY)],
            checks: &[SessionCheck {
                what: "nothing in the visit is what the scanner called its output",
                scheme: SAME_DAY,
                labels: &["20220901"],
                flagged: 0,
                primaries: &["no"],
            }],
            fingerprint: None,
        },
        ResCase {
            name: "res-has-primary",
            what: "the same visit with one primary in it",
            needs: "no rescue: one primary anywhere answers for the whole visit",
            studies: &[("20220901", SECONDARY), ("20220901", PRIMARY)],
            checks: &[SessionCheck {
                what: "one study saying yes settles it for the others",
                scheme: SAME_DAY,
                labels: &["20220901"],
                flagged: 0,
                primaries: &["yes"],
            }],
            fingerprint: None,
        },
        ResCase {
            name: "res-window",
            what: "a brain study on the Monday and a spine study on the Wednesday",
            needs: "the scheme decides: two occasions rescue the brain study, one does not",
            studies: &[("20221003", SECONDARY), ("20221005", PRIMARY)],
            checks: &[
                SessionCheck {
                    what: "v0 groups by the calendar day, so the brain study is rescued",
                    scheme: SAME_DAY,
                    labels: &["20221003", "20221005"],
                    flagged: 0,
                    primaries: &["no", "yes"],
                },
                SessionCheck {
                    what: "one visit holds a primary, so there is nothing to rescue",
                    scheme: A_FORTNIGHT,
                    labels: &["20221003"],
                    flagged: 0,
                    primaries: &["yes"],
                },
            ],
            fingerprint: None,
        },
        ResCase {
            name: "res-screenshot-only",
            what: "a visit whose only images are screen captures",
            needs: "no primary, and nothing rescuable either: both facts, separately",
            studies: &[("20220901", "ORIGINAL\\SECONDARY\\SCREENSHOT")],
            checks: &[SessionCheck {
                what: "the occasion says no primary, and the stack says it is not an image",
                scheme: SAME_DAY,
                labels: &["20220901"],
                flagged: 0,
                primaries: &["no"],
            }],
            fingerprint: Some(FingerprintWant {
                field_strength_tesla: Some("3.0"),
                field_strength_normalized: Some("3.0"),
                field_strength_unit: Some("tesla"),
                acquisition_type_filled: None,
                acquisition_type_source: None,
                image_role: "not_an_image",
                dwi_b_value: None,
                dwi_b_values: None,
                dwi_b_value_source: None,
                dwi_pe_direction: None,
                dwi_pe_direction_source: None,
                dwi_directions: None,
                dwi_directions_source: None,
            }),
        },
    ];

    for (n, c) in rescues.iter().enumerate() {
        let mut studies = Vec::new();
        for (v, (day, image_type)) in c.studies.iter().enumerate() {
            let dir = format!("{}/RES{:03}/visit{}", c.name, n + 1, v + 1);
            let su = uid(&["13", &n.to_string(), &v.to_string()]);
            let (d, it): (&'static str, &'static str) = (day, image_type);
            let f = study(
                root,
                &dir,
                &su,
                &format!("13-{n}-{v}"),
                2,
                px,
                move |f, _| {
                    f.patient_id = Some("RESCASE");
                    f.study_date = Some(d);
                    f.series_date = Some(d);
                    f.acquisition_date = Some(d);
                    f.content_date = Some(d);
                    f.image_type = it;
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
            sessions: c
                .checks
                .iter()
                .map(|k| SessionWant {
                    what: k.what,
                    scheme: k.scheme,
                    labels: k.labels.to_vec(),
                    flagged: k.flagged,
                    primaries: k.primaries.to_vec(),
                })
                .collect(),
            fingerprint: c.fingerprint.as_ref().map(|w| FingerprintWant { ..*w }),
        });
    }

    // ------------------------------------------------------------ diffusion

    // §6. Each is one series whose images differ, because that is the whole
    // point: v0 reads one row per series and so records every acquisition as
    // one shell and one direction.
    struct DwiCase {
        name: &'static str,
        what: &'static str,
        needs: &'static str,
        /// One entry per image: b value, gradient, directionality.
        images: &'static [(&'static str, &'static str, &'static str)],
        description: &'static str,
        /// The Siemens private b value per image, when the scenario uses it.
        siemens: &'static [&'static str],
        geometry: Option<(&'static str, &'static str, &'static str)>,
        want: FingerprintWant,
    }

    fn dwi_want(
        b_value: Option<&'static str>,
        b_values: Option<&'static str>,
        b_source: Option<&'static str>,
        pe: Option<&'static str>,
        pe_source: Option<&'static str>,
        n: Option<&'static str>,
        n_source: Option<&'static str>,
    ) -> FingerprintWant {
        FingerprintWant {
            field_strength_tesla: Some("3.0"),
            field_strength_normalized: Some("3.0"),
            field_strength_unit: Some("tesla"),
            acquisition_type_filled: None,
            acquisition_type_source: None,
            image_role: "original_primary",
            dwi_b_value: b_value,
            dwi_b_values: b_values,
            dwi_b_value_source: b_source,
            dwi_pe_direction: pe,
            dwi_pe_direction_source: pe_source,
            dwi_directions: n,
            dwi_directions_source: n_source,
        }
    }

    let dwis = [
        DwiCase {
            name: "dwi-two-shells",
            what: "one series holding b=0 and b=1000",
            needs: "both shells; v0 reads one row per series and sees one",
            images: &[("0", "", "NONE"), ("1000", "1\\0\\0", "DIRECTIONAL")],
            description: "ax dwi",
            siemens: &[],
            geometry: None,
            want: dwi_want(
                Some("1000"),
                Some("0,1000"),
                Some("standard"),
                None,
                None,
                Some("1"),
                Some("gradients"),
            ),
        },
        DwiCase {
            name: "dwi-directions",
            what: "six gradient directions and a b0",
            needs: "six; v0 reports one for every stack that has a gradient at all",
            images: &[
                ("0", "0\\0\\0", "NONE"),
                ("1000", "1\\0\\0", "DIRECTIONAL"),
                ("1000", "0\\1\\0", "DIRECTIONAL"),
                ("1000", "0\\0\\1", "DIRECTIONAL"),
                ("1000", "1\\1\\0", "DIRECTIONAL"),
                ("1000", "0\\1\\1", "DIRECTIONAL"),
                ("1000", "1\\0\\1", "DIRECTIONAL"),
            ],
            description: "dti 6 riktningar",
            siemens: &[],
            geometry: None,
            want: dwi_want(
                Some("1000"),
                Some("0,1000"),
                Some("standard"),
                None,
                None,
                Some("6"),
                Some("gradients"),
            ),
        },
        DwiCase {
            name: "dwi-trace-keeps-its-shell",
            what: "a Trace image the scanner wrote b=0 on",
            needs: "1000 from the sequence name, because b=0 alone is ambiguous",
            images: &[("0", "", "ISOTROPIC")],
            description: "ax dwi trace *re_b1000t",
            siemens: &[],
            geometry: None,
            want: dwi_want(
                Some("1000"),
                Some("0,1000"),
                Some("standard,text"),
                None,
                None,
                None,
                None,
            ),
        },
        DwiCase {
            name: "dwi-private-and-standard",
            what: "the same shells written in both the private and the standard tag",
            needs: "one set of shells, and both sources named",
            images: &[("0", "", "NONE"), ("1000", "1\\0\\0", "DIRECTIONAL")],
            description: "ep2d_diff",
            siemens: &["0", "1000"],
            geometry: None,
            want: dwi_want(
                Some("1000"),
                Some("0,1000"),
                Some("private,standard"),
                None,
                None,
                Some("1"),
                Some("gradients"),
            ),
        },
        DwiCase {
            name: "dwi-phase-direction",
            what: "an axial slice with the phase encoding along the column",
            needs: "AP from the cosines; v0 always takes the column, right or wrong",
            images: &[("1000", "1\\0\\0", "DIRECTIONAL")],
            description: "ep2d_diff",
            siemens: &[],
            geometry: Some(("1\\0\\0\\0\\1\\0", "COL", "1")),
            want: dwi_want(
                Some("1000"),
                Some("1000"),
                Some("standard"),
                Some("AP"),
                Some("geometry"),
                Some("1"),
                Some("gradients"),
            ),
        },
        DwiCase {
            name: "dwi-phase-from-name",
            what: "no CSA flag, and the direction written into the name",
            needs: "PA from the text, and the source says so",
            images: &[("1000", "1\\0\\0", "DIRECTIONAL")],
            description: "ep2d_diff_b1000_PA",
            siemens: &[],
            geometry: None,
            want: dwi_want(
                Some("1000"),
                Some("1000"),
                Some("standard"),
                Some("PA"),
                Some("text"),
                Some("1"),
                Some("gradients"),
            ),
        },
        DwiCase {
            name: "dwi-not-diffusion",
            what: "an anatomical series whose name holds a number between underscores",
            needs: "nothing derived: the loose patterns are only asked once something says diffusion",
            images: &[("", "", "")],
            description: "t1_mprage_32_sag",
            siemens: &[],
            geometry: None,
            want: dwi_want(None, None, None, None, None, None, None),
        },
    ];

    for (n, c) in dwis.iter().enumerate() {
        let dir = format!("{}/DWI{:03}/visit1", c.name, n + 1);
        let su = uid(&["14", &n.to_string()]);
        let images = c.images;
        let siemens = c.siemens;
        let geometry = c.geometry;
        let description = c.description;
        let f = study(
            root,
            &dir,
            &su,
            &format!("14-{n}"),
            images.len() as u64,
            px,
            move |f, i| {
                f.patient_id = Some("DWICASE");
                f.series_description = description;
                let (b, g, d) = images[i as usize];
                f.b_value = (!b.is_empty()).then(|| b.to_string());
                f.gradient = (!g.is_empty()).then(|| g.to_string());
                f.directionality = (!d.is_empty()).then_some(d);
                f.siemens_b_value = siemens.get(i as usize).map(|v| (*v).to_string());
                if let Some((iop, in_plane, positive)) = geometry {
                    f.iop = Some(iop);
                    f.in_plane = Some(in_plane);
                    f.pe_positive = Some(positive);
                }
            },
        );
        seeds.push((c.name, f));
        scenarios.push(Scenario {
            name: c.name,
            what: c.what,
            people: 1,
            needs: c.needs,
            studies: vec![StudyWant {
                dir,
                date: Some("20220115"),
                source: "study_date",
                person: "P1".into(),
            }],
            sessions: Vec::new(),
            fingerprint: Some(FingerprintWant { ..c.want }),
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
        writeln!(out, "      ],").unwrap();
        match &s.fingerprint {
            Some(w) => {
                let q = |v: Option<&str>| match v {
                    Some(t) => format!("{t:?}"),
                    None => "null".to_string(),
                };
                writeln!(
                    out,
                    "      \"fingerprint\": {{\"field_strength_tesla\": {}, \"field_strength_normalized\": {}, \
                     \"field_strength_unit\": {}, \"acquisition_type_filled\": {}, \
                     \"acquisition_type_source\": {}, \"image_role\": {:?}, \
                     \"dwi_b_value\": {}, \"dwi_b_values\": {}, \"dwi_b_value_source\": {}, \
                     \"dwi_pe_direction\": {}, \"dwi_pe_direction_source\": {}, \
                     \"dwi_directions\": {}, \"dwi_directions_source\": {}}},",
                    q(w.field_strength_tesla),
                    q(w.field_strength_normalized),
                    q(w.field_strength_unit),
                    q(w.acquisition_type_filled),
                    q(w.acquisition_type_source),
                    w.image_role,
                    q(w.dwi_b_value),
                    q(w.dwi_b_values),
                    q(w.dwi_b_value_source),
                    q(w.dwi_pe_direction),
                    q(w.dwi_pe_direction_source),
                    q(w.dwi_directions),
                    q(w.dwi_directions_source)
                )
                .unwrap();
            }
            None => writeln!(out, "      \"fingerprint\": null,").unwrap(),
        }
        writeln!(out, "      \"sessions\": [").unwrap();
        for (j, w) in s.sessions.iter().enumerate() {
            let labels: Vec<String> = w.labels.iter().map(|l| format!("{l:?}")).collect();
            let primaries: Vec<String> = w.primaries.iter().map(|l| format!("{l:?}")).collect();
            writeln!(
                out,
                "        {{\"what\": {:?}, \"scheme\": {:?}, \"flagged\": {}, \"labels\": [{}], \
                 \"primaries\": [{}]}}{}",
                w.what,
                w.scheme,
                w.flagged,
                labels.join(", "),
                primaries.join(", "),
                if j + 1 == s.sessions.len() { "" } else { "," }
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
