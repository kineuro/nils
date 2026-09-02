// SPDX-License-Identifier: AGPL-3.0-only

//! Write a synthetic corpus: subjects, studies, series and instances that
//! never existed, from a seed, for the digest's tests and benchmarks
//! (`docs/specs/wave1-parse-and-digest.md`, §12.6; design record, C10).
//!
//! ```sh
//! cargo run --release -p nils-dicom --example corpus -- \
//!     --out /scratch/nils/synth --instances 1000000 --seed 1 > synth-manifest.json
//! ```
//!
//! What the tree holds, so a digest of it exercises every path the spec
//! names: MR, CT and PT series in v0's proportions, a share of Enhanced MR
//! (one multi-frame file per series, its parameters in the functional
//! groups), Part 10 files with and without the preamble and bare data sets,
//! explicit and implicit VR, three character sets, subjects without a
//! PatientID (identity falls to the study), duplicates under a second path,
//! and files the reader refuses (empty, truncated, text, junk, no SOP
//! Instance UID, an unsupported modality). Nothing in it derives from a real
//! registry; the values come from the seed alone.
//!
//! `--instances N` is exact: N accepted instance files, plus the duplicates
//! and the refused files, whose counts are printed at the end as JSON so a
//! run can check the digest's report against them.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use dicom_core::{Tag, VR};
use dicom_dictionary_std::tags;
use nils_dicom::read::{EXPLICIT_VR_LE, IMPLICIT_VR_LE};
use nils_dicom::synth::{self, Elem, MetaFields};

/// The UID root of everything synthetic (the DICOM example root).
const ROOT: &str = "1.2.826.0.1.3680043.8.498";

const MR_IMAGE: &str = "1.2.840.10008.5.1.4.1.1.4";
const ENHANCED_MR: &str = "1.2.840.10008.5.1.4.1.1.4.1";
const CT_IMAGE: &str = "1.2.840.10008.5.1.4.1.1.2";
const PET_IMAGE: &str = "1.2.840.10008.5.1.4.1.1.128";
const US_IMAGE: &str = "1.2.840.10008.5.1.4.1.1.6.1";

struct Options {
    out: PathBuf,
    instances: u64,
    seed: u64,
    /// Bytes of Pixel Data appended to every accepted file.
    pixel_bytes: usize,
    /// Duplicates, as a share of the instances (percent).
    duplicate_percent: f64,
    /// One refused file per this many instances.
    refused_every: u64,
}

fn usage() -> ! {
    eprintln!(
        "usage: corpus --out DIR --instances N [--seed S] [--pixel-bytes B] [--duplicate-percent P] [--refused-every K]"
    );
    std::process::exit(2)
}

fn parse_args() -> Options {
    let mut out = None;
    let mut instances = None;
    let mut seed = 1u64;
    let mut pixel_bytes = 4096usize;
    let mut duplicate_percent = 1.0;
    let mut refused_every = 500u64;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || args.next().unwrap_or_else(|| usage());
        match a.as_str() {
            "--out" => out = Some(PathBuf::from(value())),
            "--instances" => instances = value().parse().ok(),
            "--seed" => seed = value().parse().unwrap_or_else(|_| usage()),
            "--pixel-bytes" => pixel_bytes = value().parse().unwrap_or_else(|_| usage()),
            "--duplicate-percent" => {
                duplicate_percent = value().parse().unwrap_or_else(|_| usage())
            }
            "--refused-every" => refused_every = value().parse().unwrap_or_else(|_| usage()),
            _ => usage(),
        }
    }
    let (Some(out), Some(instances)) = (out, instances) else {
        usage()
    };
    Options {
        out,
        instances,
        seed,
        pixel_bytes,
        duplicate_percent,
        refused_every: refused_every.max(1),
    }
}

/// splitmix64: small, fast, and the same everywhere.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// In `lo..=hi`.
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }

    fn chance(&mut self, percent: f64) -> bool {
        (self.next() % 10_000) as f64 <= percent * 100.0 - 1.0
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next() % items.len() as u64) as usize]
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Modality {
    Mr,
    EnhancedMr,
    Ct,
    Pt,
}

impl Modality {
    fn code(self) -> &'static str {
        match self {
            Modality::Mr | Modality::EnhancedMr => "MR",
            Modality::Ct => "CT",
            Modality::Pt => "PT",
        }
    }

    fn sop_class(self) -> &'static str {
        match self {
            Modality::Mr => MR_IMAGE,
            Modality::EnhancedMr => ENHANCED_MR,
            Modality::Ct => CT_IMAGE,
            Modality::Pt => PET_IMAGE,
        }
    }
}

#[derive(Clone, Copy)]
enum Charset {
    None,
    Latin1,
    Utf8,
}

struct Subject {
    n: u64,
    patient_id: Option<String>,
    name: String,
    birth: String,
    sex: &'static str,
}

struct Study {
    uid: String,
    date: String,
    time: String,
    description: &'static str,
    manufacturer: &'static str,
    model: &'static str,
    station: String,
    institution: &'static str,
    charset: Charset,
}

struct Series {
    uid: String,
    number: u64,
    modality: Modality,
    description: String,
    body_part: &'static str,
    protocol: String,
    frame_of_reference: String,
    thickness: f64,
    spacing: f64,
    orientation: &'static str,
    position: &'static str,
    // MR
    tr: f64,
    te: f64,
    ti: Option<f64>,
    flip: f64,
    field: &'static str,
    coil: &'static str,
    matrix: &'static str,
    pe_dir: &'static str,
    etl: u64,
    scanning: &'static str,
    variant: &'static str,
    sequence_name: &'static str,
    b_value: Option<u64>,
    // CT
    kvp: u64,
    current: u64,
    exposure: u64,
    kernel: &'static str,
    pitch: f64,
    // PT
    tracer: &'static str,
    dose: f64,
    half_life: f64,
    units: &'static str,
    rows: u64,
    columns: u64,
    pixel_spacing: String,
}

/// The counts printed at the end.
#[derive(Default)]
struct Tally {
    subjects: u64,
    subjects_without_patient_id: u64,
    studies: u64,
    series: u64,
    series_mr: u64,
    series_enhanced_mr: u64,
    series_ct: u64,
    series_pt: u64,
    instances: u64,
    duplicates: u64,
    refused: u64,
    refused_empty: u64,
    refused_truncated: u64,
    refused_text: u64,
    refused_junk: u64,
    refused_missing_uid: u64,
    refused_unsupported_modality: u64,
    part10_with_preamble: u64,
    part10_without_preamble: u64,
    bare: u64,
    implicit_vr: u64,
    charset_none: u64,
    charset_latin1: u64,
    charset_utf8: u64,
    bytes: u64,
}

fn main() {
    let opts = parse_args();
    let start = Instant::now();
    fs::create_dir_all(&opts.out).expect("create the output directory");
    let mut rng = Rng(opts.seed ^ 0xA5A5_5A5A_1234_5678);
    let mut tally = Tally::default();
    let mut refused_due = opts.refused_every;
    let mut next_refused = 0u64;
    let pixels = vec![0x80u8; opts.pixel_bytes];

    while tally.instances < opts.instances {
        let subject = subject(&mut rng, tally.subjects + 1);
        tally.subjects += 1;
        if subject.patient_id.is_none() {
            tally.subjects_without_patient_id += 1;
        }
        let studies = rng.range(1, 3);
        for s in 1..=studies {
            if tally.instances >= opts.instances {
                break;
            }
            let study = study(&mut rng, &subject, s);
            tally.studies += 1;
            let n_series = rng.range(1, 8);
            for k in 1..=n_series {
                if tally.instances >= opts.instances {
                    break;
                }
                let series = series(&mut rng, &study, k);
                tally.series += 1;
                match series.modality {
                    Modality::Mr => tally.series_mr += 1,
                    Modality::EnhancedMr => {
                        tally.series_mr += 1;
                        tally.series_enhanced_mr += 1;
                    }
                    Modality::Ct => tally.series_ct += 1,
                    Modality::Pt => tally.series_pt += 1,
                }
                let dir = opts.out.join(format!(
                    "sub-{:06}/st-{}/se-{:02}-{}",
                    subject.n,
                    s,
                    k,
                    series.modality.code()
                ));
                fs::create_dir_all(&dir).expect("create the series directory");
                let count = match series.modality {
                    Modality::EnhancedMr => 1,
                    Modality::Mr => rng.range(20, 200),
                    Modality::Ct => rng.range(50, 400),
                    Modality::Pt => rng.range(40, 300),
                };
                let count = count.min(opts.instances - tally.instances);
                let extension = rng.chance(40.0);
                for i in 1..=count {
                    let name = if extension {
                        format!("IM_{i:04}.dcm")
                    } else {
                        format!("IM_{i:04}")
                    };
                    let bytes =
                        instance(&mut rng, &subject, &study, &series, i, &pixels, &mut tally);
                    let path = dir.join(&name);
                    write(&path, &bytes);
                    tally.instances += 1;
                    tally.bytes += bytes.len() as u64;
                    if rng.chance(opts.duplicate_percent) {
                        let copy = opts
                            .out
                            .join("dup")
                            .join(path.strip_prefix(&opts.out).expect("under out"));
                        fs::create_dir_all(copy.parent().expect("parent"))
                            .expect("create the duplicate's directory");
                        write(&copy, &bytes);
                        tally.duplicates += 1;
                        tally.bytes += bytes.len() as u64;
                    }
                    refused_due -= 1;
                    if refused_due == 0 {
                        refused_due = opts.refused_every;
                        next_refused += 1;
                        refused(&mut rng, &dir, next_refused, &bytes, &mut tally);
                    }
                }
            }
        }
    }

    let elapsed = start.elapsed().as_secs_f64();
    let doc = serde_json::json!({
        "seed": opts.seed,
        "root": opts.out.display().to_string(),
        "elapsed_s": (elapsed * 10.0).round() / 10.0,
        "files": tally.instances + tally.duplicates + tally.refused,
        "bytes": tally.bytes,
        "subjects": tally.subjects,
        "subjects_without_patient_id": tally.subjects_without_patient_id,
        "studies": tally.studies,
        "series": tally.series,
        "series_mr": tally.series_mr,
        "series_enhanced_mr": tally.series_enhanced_mr,
        "series_ct": tally.series_ct,
        "series_pt": tally.series_pt,
        "instances": tally.instances,
        "duplicates": tally.duplicates,
        "refused": tally.refused,
        "refused_by_class": {
            "not_dicom": tally.refused_empty + tally.refused_text + tally.refused_junk,
            "parse_error": tally.refused_truncated,
            "missing_uid": tally.refused_missing_uid,
            "unsupported_modality": tally.refused_unsupported_modality,
        },
        "forms": {
            "part10_with_preamble": tally.part10_with_preamble,
            "part10_without_preamble": tally.part10_without_preamble,
            "bare": tally.bare,
        },
        "implicit_vr": tally.implicit_vr,
        "charsets": {
            "none": tally.charset_none,
            "ISO_IR 100": tally.charset_latin1,
            "ISO_IR 192": tally.charset_utf8,
        },
    });
    // to stdout only: a manifest inside the tree would be one more refused file
    println!(
        "{}",
        serde_json::to_string_pretty(&doc).expect("render the manifest")
    );
}

fn write(path: &Path, bytes: &[u8]) {
    let mut f = fs::File::create(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    f.write_all(bytes)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

const GIVEN: &[&str] = &[
    "Alva", "Bo", "Cleo", "Dag", "Elin", "Finn", "Greta", "Hugo", "Ida", "Jon", "Kai", "Liv",
    "Moa", "Nils", "Otto", "Pia", "Rut", "Sixten", "Tove", "Ulf", "Vera", "Wilma", "Ylva", "Zed",
];
const FAMILY: &[&str] = &[
    "Andersson",
    "Berg",
    "Cedergren",
    "Dahl",
    "Ek",
    "Forsberg",
    "Gran",
    "Holm",
    "Isaksson",
    "Jonsson",
    "Kvist",
    "Lind",
    "Malm",
    "Nyberg",
    "Olsson",
    "Palm",
    "Qvarnström",
    "Rask",
    "Sjöberg",
    "Tegnér",
    "Udd",
    "Vinter",
    "Wall",
    "Ågren",
    "Öman",
];
const INSTITUTIONS: &[&str] = &[
    "Synthetic Hospital North",
    "Synthetic Hospital South",
    "Synthetic Imaging Centre",
    "Synthetic University Clinic",
];
const MANUFACTURERS: &[(&str, &[&str])] = &[
    (
        "SIEMENS",
        &[
            "Prisma",
            "Skyra",
            "Avanto",
            "Vida",
            "SOMATOM Force",
            "Biograph mCT",
        ],
    ),
    (
        "GE MEDICAL SYSTEMS",
        &[
            "SIGNA Premier",
            "DISCOVERY MR750",
            "Revolution CT",
            "Discovery MI",
        ],
    ),
    (
        "Philips",
        &["Ingenia", "Achieva", "Brilliance 64", "Vereos"],
    ),
    ("Canon", &["Vantage Galan", "Aquilion ONE"]),
];
const STUDY_DESCRIPTIONS: &[&str] = &[
    "MR Brain",
    "MR Brain with contrast",
    "MR Spine",
    "CT Head",
    "CT Thorax",
    "PET CT Whole body",
    "Research protocol A",
    "Research protocol B",
    "Hjärna MR",
    "Rygg MR",
];
const BODY_PARTS: &[&str] = &["BRAIN", "HEAD", "SPINE", "CHEST", "NECK", "WHOLEBODY"];
const MR_SERIES: &[(&str, &str, &str, &str, &str)] = &[
    // description, sequence name, scanning sequence, variant, protocol
    (
        "t1_mprage_sag",
        "*tfl3d1_16ns",
        "GR\\IR",
        "SP\\MP",
        "t1_mprage_sag_p2_iso",
    ),
    ("t2_tse_tra", "*tse2d1_11", "SE", "SK\\SP", "t2_tse_tra_p2"),
    (
        "t2_flair_sag",
        "*spcir_278ns",
        "SE\\IR",
        "SK\\SP\\MP",
        "t2_space_dark-fluid_sag_p2",
    ),
    (
        "ep2d_diff_b1000",
        "*ep_b1000#1",
        "EP",
        "SK\\SP",
        "ep2d_diff_mddw_20",
    ),
    ("swi_tra", "*swi3d1r", "GR", "SP", "t2_swi3d_tra_p2"),
    ("localizer", "*fl2d1", "GR", "SP\\OSP", "localizer"),
    ("rest_bold", "*epfid2d1_64", "EP", "SK", "ep2d_bold_rest"),
    ("T1 vibe dixon", "*fl3d2", "GR", "SP", "t1_vibe_dixon_tra"),
    (
        "t1_mprage_sag åäö",
        "*tfl3d1_16ns",
        "GR\\IR",
        "SP\\MP",
        "t1_mprage_sag_p2_iso",
    ),
];
const CT_SERIES: &[(&str, &str, &str)] = &[
    ("Head 5.0 H31s", "H31s", "Head Routine"),
    ("Head 1.0 H60s", "H60s", "Head Routine"),
    ("Thorax 3.0 B70f", "B70f", "Thorax"),
    ("Topogram", "T20f", "Topogram"),
    ("Angio 0.75 I26f", "I26f", "CTA Head"),
];
const PT_SERIES: &[(&str, &str, &str, f64, f64)] = &[
    ("PET WB AC", "FDG", "BQML", 250e6, 6586.2),
    ("PET WB NAC", "FDG", "BQML", 250e6, 6586.2),
    ("PET Brain", "Flutemetamol", "BQML", 185e6, 6586.2),
    ("PET Brain dynamic", "PSMA", "BQML", 200e6, 4062.0),
];
const COILS: &[&str] = &[
    "HeadNeck_64",
    "Head_32",
    "HeadNeck_20",
    "Body_18",
    "Spine_32",
];
const MATRICES: &[&str] = &[
    "256\\0\\0\\256",
    "0\\256\\256\\0",
    "384\\0\\0\\384",
    "128\\0\\0\\128",
];
const ORIENTATIONS: &[&str] = &[
    "1\\0\\0\\0\\1\\0",
    "1\\0\\0\\0\\0\\-1",
    "0\\1\\0\\0\\0\\-1",
    "0.998\\0\\-0.058\\0\\1\\0",
];
const STATIONS: &[&str] = &["MRC1", "MRC2", "CT01", "CT02", "PET1"];

fn subject(rng: &mut Rng, n: u64) -> Subject {
    let year = rng.range(1930, 2012);
    let month = rng.range(1, 12);
    let day = rng.range(1, 28);
    let given = rng.pick(GIVEN);
    let family = rng.pick(FAMILY);
    Subject {
        n,
        patient_id: if rng.chance(2.0) {
            None
        } else {
            Some(format!("SYN{:07}", rng.range(1, 9_999_999)))
        },
        name: format!("{family}^{given}"),
        birth: format!("{year:04}{month:02}{day:02}"),
        sex: rng.pick(&["M", "F", "O"]),
    }
}

fn study(rng: &mut Rng, subject: &Subject, s: u64) -> Study {
    let (manufacturer, models) = rng.pick(MANUFACTURERS);
    let year = rng.range(2015, 2025);
    let month = rng.range(1, 12);
    let day = rng.range(1, 28);
    Study {
        uid: format!("{ROOT}.{}.{s}", subject.n),
        date: format!("{year:04}{month:02}{day:02}"),
        time: format!(
            "{:02}{:02}{:02}",
            rng.range(7, 19),
            rng.range(0, 59),
            rng.range(0, 59)
        ),
        description: rng.pick(STUDY_DESCRIPTIONS),
        manufacturer,
        model: rng.pick(models),
        station: rng.pick(STATIONS).to_string(),
        institution: rng.pick(INSTITUTIONS),
        charset: if rng.chance(85.0) {
            Charset::Latin1
        } else if rng.chance(66.0) {
            Charset::Utf8
        } else {
            Charset::None
        },
    }
}

fn series(rng: &mut Rng, study: &Study, k: u64) -> Series {
    let modality = {
        let r = rng.unit();
        if r < 0.63 {
            Modality::Mr
        } else if r < 0.70 {
            Modality::EnhancedMr
        } else if r < 0.90 {
            Modality::Ct
        } else {
            Modality::Pt
        }
    };
    let uid = format!("{}.{k}", study.uid);
    let mut s = Series {
        uid,
        number: k * 100 + rng.range(0, 9),
        modality,
        description: String::new(),
        body_part: rng.pick(BODY_PARTS),
        protocol: String::new(),
        frame_of_reference: format!("{}.{k}.0", study.uid),
        thickness: *rng.pick(&[0.9, 1.0, 2.0, 3.0, 5.0]),
        spacing: *rng.pick(&[1.0, 1.2, 3.0, 3.3, 5.0]),
        orientation: rng.pick(ORIENTATIONS),
        position: rng.pick(&["HFS", "HFP", "FFS"]),
        tr: *rng.pick(&[2300.0, 3000.0, 8000.0, 9000.0, 500.0, 2000.0]),
        te: *rng.pick(&[2.98, 3.4, 90.0, 100.0, 20.0, 30.0]),
        ti: if rng.chance(30.0) { Some(900.0) } else { None },
        flip: *rng.pick(&[9.0, 90.0, 120.0, 150.0, 15.0]),
        field: rng.pick(&["1.5", "3", "7"]),
        coil: rng.pick(COILS),
        matrix: rng.pick(MATRICES),
        pe_dir: rng.pick(&["ROW", "COL"]),
        etl: rng.range(1, 32),
        scanning: "",
        variant: "",
        sequence_name: "",
        b_value: None,
        kvp: *rng.pick(&[80, 100, 120, 140]),
        current: rng.range(50, 600),
        exposure: rng.range(20, 400),
        kernel: "",
        pitch: *rng.pick(&[0.55, 0.8, 1.0, 1.2]),
        tracer: "",
        dose: 0.0,
        half_life: 0.0,
        units: "",
        rows: 256,
        columns: 256,
        pixel_spacing: String::new(),
    };
    match modality {
        Modality::Mr | Modality::EnhancedMr => {
            let (desc, seq, scanning, variant, protocol) = rng.pick(MR_SERIES);
            s.description = desc.to_string();
            s.sequence_name = seq;
            s.scanning = scanning;
            s.variant = variant;
            s.protocol = protocol.to_string();
            if *scanning == "EP" {
                s.b_value = Some(*rng.pick(&[0, 1000, 2000, 3000]));
            }
            let m = *rng.pick(&[192u64, 256, 320, 384, 512]);
            s.rows = m;
            s.columns = m;
            let ps = 240.0 / m as f64;
            s.pixel_spacing = format!("{ps:.4}\\{ps:.4}");
        }
        Modality::Ct => {
            let (desc, kernel, protocol) = rng.pick(CT_SERIES);
            s.description = desc.to_string();
            s.kernel = kernel;
            s.protocol = protocol.to_string();
            s.rows = 512;
            s.columns = 512;
            let ps = *rng.pick(&[0.39, 0.45, 0.68, 0.98]);
            s.pixel_spacing = format!("{ps}\\{ps}");
        }
        Modality::Pt => {
            let (desc, tracer, units, dose, half_life) = rng.pick(PT_SERIES);
            s.description = desc.to_string();
            s.tracer = tracer;
            s.units = units;
            s.dose = *dose;
            s.half_life = *half_life;
            s.protocol = "PET WB".to_string();
            let m = *rng.pick(&[128u64, 200, 256, 400]);
            s.rows = m;
            s.columns = m;
            let ps = 600.0 / m as f64;
            s.pixel_spacing = format!("{ps:.3}\\{ps:.3}");
        }
    }
    s
}

/// A text element in the study's character set: Latin-1 bytes for the
/// default repertoire and ISO_IR 100, UTF-8 for ISO_IR 192. Characters
/// beyond Latin-1 only appear with UTF-8.
fn t(charset: Charset, tag: Tag, vr: VR, value: &str) -> Elem {
    match charset {
        Charset::Utf8 => synth::bytes(tag, vr, value.as_bytes().to_vec()),
        _ => synth::text(tag, vr, value),
    }
}

fn ds(value: f64) -> String {
    let s = format!("{value}");
    if s.len() > 16 { s[..16].to_string() } else { s }
}

fn common(
    charset: Charset,
    subject: &Subject,
    study: &Study,
    series: &Series,
    sop_class: &str,
    sop: &str,
) -> Vec<Elem> {
    let mut e = vec![
        synth::text(tags::SOP_CLASS_UID, VR::UI, sop_class),
        synth::text(tags::SOP_INSTANCE_UID, VR::UI, sop),
        synth::text(tags::STUDY_INSTANCE_UID, VR::UI, &study.uid),
        synth::text(tags::SERIES_INSTANCE_UID, VR::UI, &series.uid),
        synth::text(tags::MODALITY, VR::CS, series.modality.code()),
        synth::text(tags::STUDY_DATE, VR::DA, &study.date),
        synth::text(tags::STUDY_TIME, VR::TM, &study.time),
        synth::text(tags::SERIES_DATE, VR::DA, &study.date),
        synth::text(tags::SERIES_TIME, VR::TM, &study.time),
        t(charset, tags::STUDY_DESCRIPTION, VR::LO, study.description),
        synth::text(tags::MANUFACTURER, VR::LO, study.manufacturer),
        synth::text(tags::MANUFACTURER_MODEL_NAME, VR::LO, study.model),
        synth::text(tags::STATION_NAME, VR::SH, &study.station),
        t(charset, tags::INSTITUTION_NAME, VR::LO, study.institution),
        t(charset, tags::PATIENT_NAME, VR::PN, &subject.name),
        synth::text(tags::PATIENT_BIRTH_DATE, VR::DA, &subject.birth),
        synth::text(tags::PATIENT_SEX, VR::CS, subject.sex),
        t(
            charset,
            tags::SERIES_DESCRIPTION,
            VR::LO,
            &series.description,
        ),
        synth::text(tags::SERIES_NUMBER, VR::IS, &series.number.to_string()),
        synth::text(tags::BODY_PART_EXAMINED, VR::CS, series.body_part),
        synth::text(tags::PROTOCOL_NAME, VR::LO, &series.protocol),
        synth::text(
            tags::FRAME_OF_REFERENCE_UID,
            VR::UI,
            &series.frame_of_reference,
        ),
        synth::text(tags::PATIENT_POSITION, VR::CS, series.position),
        synth::text(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
        synth::us(tags::ROWS, series.rows as u16),
        synth::us(tags::COLUMNS, series.columns as u16),
        synth::us(tags::BITS_ALLOCATED, 16),
        synth::us(tags::BITS_STORED, 12),
        synth::us(tags::HIGH_BIT, 11),
        synth::us(tags::PIXEL_REPRESENTATION, 0),
        synth::us(tags::SAMPLES_PER_PIXEL, 1),
        synth::text(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
    ];
    match charset {
        Charset::None => {}
        Charset::Latin1 => e.push(synth::text(
            tags::SPECIFIC_CHARACTER_SET,
            VR::CS,
            "ISO_IR 100",
        )),
        Charset::Utf8 => e.push(synth::text(
            tags::SPECIFIC_CHARACTER_SET,
            VR::CS,
            "ISO_IR 192",
        )),
    }
    if let Some(id) = &subject.patient_id {
        e.push(synth::text(tags::PATIENT_ID, VR::LO, id));
    }
    e
}

fn mr_elems(series: &Series) -> Vec<Elem> {
    let mut e = vec![
        synth::text(tags::SCANNING_SEQUENCE, VR::CS, series.scanning),
        synth::text(tags::SEQUENCE_VARIANT, VR::CS, series.variant),
        synth::text(tags::SCAN_OPTIONS, VR::CS, "PFP\\FS"),
        synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, "2D"),
        synth::text(tags::SEQUENCE_NAME, VR::SH, series.sequence_name),
        synth::text(tags::ANGIO_FLAG, VR::CS, "N"),
        synth::text(tags::SLICE_THICKNESS, VR::DS, &ds(series.thickness)),
        synth::text(tags::REPETITION_TIME, VR::DS, &ds(series.tr)),
        synth::text(tags::ECHO_TIME, VR::DS, &ds(series.te)),
        synth::text(tags::NUMBER_OF_AVERAGES, VR::DS, "1"),
        synth::text(tags::IMAGING_FREQUENCY, VR::DS, "123.2"),
        synth::text(tags::IMAGED_NUCLEUS, VR::SH, "1H"),
        synth::text(tags::ECHO_NUMBERS, VR::IS, "1"),
        synth::text(tags::MAGNETIC_FIELD_STRENGTH, VR::DS, series.field),
        synth::text(tags::SPACING_BETWEEN_SLICES, VR::DS, &ds(series.spacing)),
        synth::text(tags::NUMBER_OF_PHASE_ENCODING_STEPS, VR::IS, "256"),
        synth::text(tags::ECHO_TRAIN_LENGTH, VR::IS, &series.etl.to_string()),
        synth::text(tags::PERCENT_SAMPLING, VR::DS, "100"),
        synth::text(tags::PERCENT_PHASE_FIELD_OF_VIEW, VR::DS, "100"),
        synth::text(tags::PIXEL_BANDWIDTH, VR::DS, "240"),
        synth::text(tags::RECEIVE_COIL_NAME, VR::SH, series.coil),
        synth::text(tags::TRANSMIT_COIL_NAME, VR::SH, "Body"),
        acquisition_matrix(series.matrix),
        synth::text(
            tags::IN_PLANE_PHASE_ENCODING_DIRECTION,
            VR::CS,
            series.pe_dir,
        ),
        synth::text(tags::FLIP_ANGLE, VR::DS, &ds(series.flip)),
        synth::text(tags::SAR, VR::DS, "0.3"),
        synth::text(tags::D_BDT, VR::DS, "0"),
    ];
    if let Some(ti) = series.ti {
        e.push(synth::text(tags::INVERSION_TIME, VR::DS, &ds(ti)));
    }
    if let Some(b) = series.b_value {
        e.push(synth::num(tags::DIFFUSION_B_VALUE, VR::FD, b as f64));
        e.push(synth::text(
            tags::DIFFUSION_DIRECTIONALITY,
            VR::CS,
            "DIRECTIONAL",
        ));
        // the Siemens private block too, as v0's corpora carry it
        e.push(synth::text(
            Tag(0x0019, 0x0010),
            VR::LO,
            "SIEMENS MR HEADER",
        ));
        e.push(synth::text(Tag(0x0019, 0x100C), VR::IS, &b.to_string()));
        e.push(synth::text(Tag(0x0019, 0x100D), VR::CS, "DIRECTIONAL"));
    }
    e
}

/// An Enhanced MR file's functional groups: the parameters in the shared
/// groups, the positions per frame. The old attributes keep their
/// dictionary VRs (DS, IS) inside the groups, as the standard has them, so
/// an implicit VR file reads the same; only the Enhanced-only ones are FD.
fn enhanced_groups(series: &Series, frames: u64) -> Vec<Elem> {
    let shared = vec![
        synth::seq(
            tags::MR_TIMING_AND_RELATED_PARAMETERS_SEQUENCE,
            vec![vec![
                synth::text(tags::REPETITION_TIME, VR::DS, &ds(series.tr)),
                synth::text(tags::FLIP_ANGLE, VR::DS, &ds(series.flip)),
                synth::text(tags::ECHO_TRAIN_LENGTH, VR::IS, &series.etl.to_string()),
            ]],
        ),
        synth::seq(
            tags::MR_ECHO_SEQUENCE,
            vec![vec![synth::num(
                tags::EFFECTIVE_ECHO_TIME,
                VR::FD,
                series.te,
            )]],
        ),
        synth::seq(
            tags::PIXEL_MEASURES_SEQUENCE,
            vec![vec![
                synth::text(tags::SLICE_THICKNESS, VR::DS, &ds(series.thickness)),
                synth::text(tags::SPACING_BETWEEN_SLICES, VR::DS, &ds(series.spacing)),
                synth::text(tags::PIXEL_SPACING, VR::DS, &series.pixel_spacing),
            ]],
        ),
        synth::seq(
            tags::PLANE_ORIENTATION_SEQUENCE,
            vec![vec![synth::text(
                tags::IMAGE_ORIENTATION_PATIENT,
                VR::DS,
                series.orientation,
            )]],
        ),
        synth::seq(
            tags::MR_RECEIVE_COIL_SEQUENCE,
            vec![vec![synth::text(
                tags::RECEIVE_COIL_NAME,
                VR::SH,
                series.coil,
            )]],
        ),
        synth::seq(
            tags::MR_TRANSMIT_COIL_SEQUENCE,
            vec![vec![synth::text(tags::TRANSMIT_COIL_NAME, VR::SH, "Body")]],
        ),
        synth::seq(
            tags::MR_AVERAGES_SEQUENCE,
            vec![vec![synth::text(tags::NUMBER_OF_AVERAGES, VR::DS, "1")]],
        ),
        synth::seq(
            tags::MRFOV_GEOMETRY_SEQUENCE,
            vec![vec![
                synth::text(tags::PERCENT_SAMPLING, VR::DS, "100"),
                synth::text(tags::PERCENT_PHASE_FIELD_OF_VIEW, VR::DS, "100"),
            ]],
        ),
        synth::seq(
            tags::MR_IMAGING_MODIFIER_SEQUENCE,
            vec![vec![synth::text(tags::PIXEL_BANDWIDTH, VR::DS, "240")]],
        ),
        synth::seq(
            tags::MR_MODIFIER_SEQUENCE,
            vec![vec![
                synth::text(tags::PARALLEL_ACQUISITION_TECHNIQUE, VR::CS, "GRAPPA"),
                synth::num(tags::PARALLEL_REDUCTION_FACTOR_IN_PLANE, VR::FD, 2.0),
            ]],
        ),
    ];
    let per_frame: Vec<Vec<Elem>> = (0..frames)
        .map(|f| {
            vec![
                synth::seq(
                    tags::PLANE_POSITION_SEQUENCE,
                    vec![vec![synth::text(
                        tags::IMAGE_POSITION_PATIENT,
                        VR::DS,
                        &format!("-120\\-120\\{}", ds(f as f64 * series.spacing)),
                    )]],
                ),
                synth::seq(
                    tags::FRAME_CONTENT_SEQUENCE,
                    vec![vec![synth::num(
                        tags::IN_STACK_POSITION_NUMBER,
                        VR::UL,
                        (f + 1) as f64,
                    )]],
                ),
            ]
        })
        .collect();
    vec![
        synth::seq(tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE, vec![shared]),
        synth::seq(tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE, per_frame),
        synth::text(tags::NUMBER_OF_FRAMES, VR::IS, &frames.to_string()),
        synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, "3D"),
        synth::text(tags::SCANNING_SEQUENCE, VR::CS, series.scanning),
        synth::text(tags::SEQUENCE_VARIANT, VR::CS, series.variant),
        synth::text(tags::MAGNETIC_FIELD_STRENGTH, VR::DS, series.field),
        synth::text(tags::IMAGED_NUCLEUS, VR::SH, "1H"),
        synth::text(tags::IMAGING_FREQUENCY, VR::DS, "123.2"),
    ]
}

fn ct_elems(series: &Series) -> Vec<Elem> {
    vec![
        synth::text(tags::KVP, VR::DS, &series.kvp.to_string()),
        synth::text(tags::SLICE_THICKNESS, VR::DS, &ds(series.thickness)),
        synth::text(tags::DATA_COLLECTION_DIAMETER, VR::DS, "500"),
        synth::text(tags::RECONSTRUCTION_DIAMETER, VR::DS, "250"),
        synth::text(tags::GANTRY_DETECTOR_TILT, VR::DS, "0"),
        synth::text(tags::TABLE_HEIGHT, VR::DS, "150"),
        synth::text(tags::ROTATION_DIRECTION, VR::CS, "CW"),
        synth::text(tags::EXPOSURE_TIME, VR::IS, "1000"),
        synth::text(
            tags::X_RAY_TUBE_CURRENT,
            VR::IS,
            &series.current.to_string(),
        ),
        synth::text(tags::EXPOSURE, VR::IS, &series.exposure.to_string()),
        synth::text(tags::FILTER_TYPE, VR::SH, "FLAT"),
        synth::text(tags::GENERATOR_POWER, VR::IS, "50"),
        synth::text(tags::FOCAL_SPOTS, VR::DS, "1.2"),
        synth::text(tags::CONVOLUTION_KERNEL, VR::SH, series.kernel),
        synth::num(tags::REVOLUTION_TIME, VR::FD, 0.5),
        synth::num(tags::SINGLE_COLLIMATION_WIDTH, VR::FD, 0.6),
        synth::num(tags::TOTAL_COLLIMATION_WIDTH, VR::FD, 38.4),
        synth::num(tags::TABLE_SPEED, VR::FD, 46.0),
        synth::num(tags::TABLE_FEED_PER_ROTATION, VR::FD, 23.0),
        synth::num(tags::SPIRAL_PITCH_FACTOR, VR::FD, series.pitch),
        synth::text(tags::EXPOSURE_MODULATION_TYPE, VR::CS, "XYZ_EC"),
        synth::num(tags::CTD_IVOL, VR::FD, 12.5),
        synth::seq(
            tags::CTDI_PHANTOM_TYPE_CODE_SEQUENCE,
            vec![vec![
                synth::text(tags::CODE_VALUE, VR::SH, "113690"),
                synth::text(tags::CODING_SCHEME_DESIGNATOR, VR::SH, "DCM"),
                synth::text(tags::CODE_MEANING, VR::LO, "IEC Head Dosimetry Phantom"),
            ]],
        ),
        synth::text(tags::RESCALE_INTERCEPT, VR::DS, "-1024"),
        synth::text(tags::RESCALE_SLOPE, VR::DS, "1"),
        synth::text(tags::WINDOW_CENTER, VR::DS, "40"),
        synth::text(tags::WINDOW_WIDTH, VR::DS, "80"),
    ]
}

fn pt_elems(series: &Series) -> Vec<Elem> {
    vec![
        synth::text(tags::SLICE_THICKNESS, VR::DS, &ds(series.thickness)),
        synth::seq(
            tags::RADIOPHARMACEUTICAL_INFORMATION_SEQUENCE,
            vec![vec![
                synth::text(tags::RADIOPHARMACEUTICAL, VR::LO, series.tracer),
                synth::text(tags::RADIONUCLIDE_TOTAL_DOSE, VR::DS, &ds(series.dose)),
                synth::text(tags::RADIONUCLIDE_HALF_LIFE, VR::DS, &ds(series.half_life)),
                synth::text(tags::RADIONUCLIDE_POSITRON_FRACTION, VR::DS, "0.97"),
                synth::text(tags::RADIOPHARMACEUTICAL_START_TIME, VR::TM, "093000"),
                synth::text(tags::RADIOPHARMACEUTICAL_VOLUME, VR::DS, "5"),
                synth::text(tags::RADIOPHARMACEUTICAL_ROUTE, VR::LO, "IV"),
            ]],
        ),
        synth::text(tags::DECAY_CORRECTION, VR::CS, "START"),
        synth::text(tags::DECAY_FACTOR, VR::DS, "1.12"),
        synth::text(tags::RECONSTRUCTION_METHOD, VR::LO, "OSEM3D 4i21s"),
        synth::text(tags::SCATTER_CORRECTION_METHOD, VR::LO, "Model-based"),
        synth::text(
            tags::ATTENUATION_CORRECTION_METHOD,
            VR::LO,
            "CT-derived mu-map",
        ),
        synth::text(tags::RANDOMS_CORRECTION_METHOD, VR::CS, "DLYD"),
        synth::text(tags::DOSE_CALIBRATION_FACTOR, VR::DS, "1.0"),
        synth::text(tags::SUV_TYPE, VR::CS, "BW"),
        synth::text(tags::COUNTS_SOURCE, VR::CS, "EMISSION"),
        synth::text(tags::UNITS, VR::CS, series.units),
        synth::text(tags::FRAME_REFERENCE_TIME, VR::DS, "0"),
        synth::text(tags::ACTUAL_FRAME_DURATION, VR::IS, "120000"),
        synth::seq(
            tags::PATIENT_GANTRY_RELATIONSHIP_CODE_SEQUENCE,
            vec![vec![
                synth::text(tags::CODE_VALUE, VR::SH, "102540008"),
                synth::text(tags::CODING_SCHEME_DESIGNATOR, VR::SH, "SCT"),
                synth::text(tags::CODE_MEANING, VR::LO, "headfirst"),
            ]],
        ),
        synth::text(tags::SLICE_PROGRESSION_DIRECTION, VR::CS, "FEET_TO_HEAD"),
        synth::text(tags::SERIES_TYPE, VR::CS, "WHOLE BODY\\IMAGE"),
        synth::us(tags::NUMBER_OF_SLICES, 1),
        synth::text(tags::RESCALE_INTERCEPT, VR::DS, "0"),
        synth::text(tags::RESCALE_SLOPE, VR::DS, "0.85"),
    ]
}

/// One instance's bytes, and the tally of the form it took.
fn instance(
    rng: &mut Rng,
    subject: &Subject,
    study: &Study,
    series: &Series,
    i: u64,
    pixels: &[u8],
    tally: &mut Tally,
) -> Vec<u8> {
    let sop = format!("{}.{i}", series.uid);
    let charset = study.charset;
    match charset {
        Charset::None => tally.charset_none += 1,
        Charset::Latin1 => tally.charset_latin1 += 1,
        Charset::Utf8 => tally.charset_utf8 += 1,
    }
    let mut e = common(
        charset,
        subject,
        study,
        series,
        series.modality.sop_class(),
        &sop,
    );
    e.push(synth::text(tags::INSTANCE_NUMBER, VR::IS, &i.to_string()));
    e.push(synth::text(tags::ACQUISITION_NUMBER, VR::IS, "1"));
    e.push(synth::text(tags::ACQUISITION_DATE, VR::DA, &study.date));
    e.push(synth::text(tags::CONTENT_DATE, VR::DA, &study.date));
    let secs = i * 2;
    let time = format!(
        "{}{:02}.{:03}",
        &study.time[..4],
        (secs % 60) as u8,
        (i * 37) % 1000
    );
    e.push(synth::text(tags::ACQUISITION_TIME, VR::TM, &time));
    e.push(synth::text(tags::CONTENT_TIME, VR::TM, &time));
    e.push(synth::text(tags::LOSSY_IMAGE_COMPRESSION, VR::CS, "00"));
    match series.modality {
        Modality::EnhancedMr => {
            let frames = rng.range(30, 200);
            e.extend(enhanced_groups(series, frames));
        }
        m => {
            let z = i as f64 * series.spacing;
            e.push(synth::text(tags::SLICE_LOCATION, VR::DS, &ds(z)));
            e.push(synth::text(
                tags::IMAGE_POSITION_PATIENT,
                VR::DS,
                &format!("-120\\-120\\{}", ds(z)),
            ));
            e.push(synth::text(
                tags::IMAGE_ORIENTATION_PATIENT,
                VR::DS,
                series.orientation,
            ));
            e.push(synth::text(
                tags::PIXEL_SPACING,
                VR::DS,
                &series.pixel_spacing,
            ));
            e.push(synth::text(tags::IMAGES_IN_ACQUISITION, VR::IS, "1"));
            match m {
                Modality::Mr => {
                    e.extend(mr_elems(series));
                    e.push(synth::text(tags::WINDOW_CENTER, VR::DS, "500"));
                    e.push(synth::text(tags::WINDOW_WIDTH, VR::DS, "1000"));
                }
                Modality::Ct => e.extend(ct_elems(series)),
                Modality::Pt => e.extend(pt_elems(series)),
                Modality::EnhancedMr => unreachable!(),
            }
        }
    }
    if !pixels.is_empty() {
        e.push(synth::bytes(tags::PIXEL_DATA, VR::OW, pixels.to_vec()));
    }

    // the form: Part 10 with the preamble, without it, or a bare data set;
    // explicit VR mostly, implicit now and then
    let r = rng.unit();
    if r < 0.05 {
        tally.bare += 1;
        synth::bare(&e, true)
    } else {
        let implicit = rng.chance(5.0);
        if implicit {
            tally.implicit_vr += 1;
        }
        let meta = MetaFields::with(
            if implicit {
                IMPLICIT_VR_LE
            } else {
                EXPLICIT_VR_LE
            },
            series.modality.sop_class(),
            &sop,
        );
        let preamble = r >= 0.15;
        if preamble {
            tally.part10_with_preamble += 1;
        } else {
            tally.part10_without_preamble += 1;
        }
        synth::part10(&meta, &e, preamble)
    }
}

/// A file the reader refuses, next to the instances, in turn: empty,
/// truncated, text, junk, no SOP Instance UID, an unsupported modality.
fn refused(rng: &mut Rng, dir: &Path, n: u64, accepted: &[u8], tally: &mut Tally) {
    tally.refused += 1;
    match n % 6 {
        0 => {
            write(&dir.join("empty.dcm"), b"");
            tally.refused_empty += 1;
        }
        1 => {
            let cut = 132 + rng.range(20, 200) as usize;
            write(
                &dir.join("IM_9999"),
                &accepted[..cut.min(accepted.len() - 1)],
            );
            tally.refused_truncated += 1;
        }
        2 => {
            write(
                &dir.join("README.txt"),
                b"Synthetic corpus for NILS; this file is not DICOM.\n",
            );
            tally.refused_text += 1;
        }
        3 => {
            let junk: Vec<u8> = (0..rng.range(300, 2000))
                .map(|_| rng.next() as u8)
                .collect();
            write(&dir.join(".DS_Store"), &junk);
            tally.refused_junk += 1;
        }
        4 => {
            let e = vec![
                synth::text(tags::SOP_CLASS_UID, VR::UI, MR_IMAGE),
                synth::text(tags::STUDY_INSTANCE_UID, VR::UI, &format!("{ROOT}.0.{n}")),
                synth::text(
                    tags::SERIES_INSTANCE_UID,
                    VR::UI,
                    &format!("{ROOT}.0.{n}.1"),
                ),
                synth::text(tags::MODALITY, VR::CS, "MR"),
            ];
            let meta = MetaFields {
                transfer_syntax: EXPLICIT_VR_LE.to_string(),
                sop_class: Some(MR_IMAGE.to_string()),
                sop_instance: None,
                implementation_class: Some(synth::IMPLEMENTATION_CLASS.to_string()),
                implementation_version: Some(synth::IMPLEMENTATION_VERSION.to_string()),
            };
            write(&dir.join("IM_9998"), &synth::part10(&meta, &e, true));
            tally.refused_missing_uid += 1;
        }
        _ => {
            let sop = format!("{ROOT}.0.{n}.1.1");
            let e = vec![
                synth::text(tags::SOP_CLASS_UID, VR::UI, US_IMAGE),
                synth::text(tags::SOP_INSTANCE_UID, VR::UI, &sop),
                synth::text(tags::STUDY_INSTANCE_UID, VR::UI, &format!("{ROOT}.0.{n}")),
                synth::text(
                    tags::SERIES_INSTANCE_UID,
                    VR::UI,
                    &format!("{ROOT}.0.{n}.1"),
                ),
                synth::text(tags::MODALITY, VR::CS, "US"),
            ];
            let meta = MetaFields::with(EXPLICIT_VR_LE, US_IMAGE, &sop);
            write(&dir.join("IM_9997.dcm"), &synth::part10(&meta, &e, true));
            tally.refused_unsupported_modality += 1;
        }
    }
}

/// AcquisitionMatrix: four unsigned shorts, written here as `a\b\c\d`.
fn acquisition_matrix(spec: &str) -> Elem {
    let mut bytes = Vec::with_capacity(8);
    for part in spec.split('\\') {
        let v: u16 = part.parse().expect("a matrix value");
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    synth::bytes(tags::ACQUISITION_MATRIX, VR::US, bytes)
}
