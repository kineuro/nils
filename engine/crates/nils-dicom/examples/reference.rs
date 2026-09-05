// SPDX-License-Identifier: AGPL-3.0-only

//! The reference selections of the release gate
//! (`docs/specs/wave3-anonymize-and-bids.md`, §12, bar 3).
//!
//! One person, two occasions, and one of every case the release has to get
//! right: a contrast BIDS names and one it does not, a scanner derivative it
//! names, a scout, a screenshot, two echoes, a second acquisition of the same
//! thing, a post-contrast repeat, and a functional series nobody has said a
//! task for.
//!
//! It is synthetic, and **its right answers are written down beside it** in a
//! manifest, so the gate asserts rather than eyeballs. The answers were read
//! off a run and checked one at a time, which is what "hand-verified" means
//! here: the generator does not compute them, so a change in the pack or the
//! grammar shows up as a difference rather than moving with the code.
//!
//! ```sh
//! cargo run --release -p nils-dicom --example reference -- --out /scratch/nils/reference
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields};

/// One series of the reference tree.
struct Series {
    /// The directory it goes in, which is also its name in the manifest.
    key: &'static str,
    study: &'static str,
    date: &'static str,
    description: &'static str,
    protocol: &'static str,
    image_type: &'static str,
    acquisition: &'static str,
    /// How many instances, and how many stacks they should split into.
    instances: usize,
    /// Extra elements, as `(group, element, VR, value)`.
    extra: &'static [(u16, u16, VR, &'static str)],
}

/// Two occasions six months apart, and everything on each.
const SERIES: &[Series] = &[
    Series {
        key: "t1-mprage",
        study: "1",
        date: "20220115",
        description: "t1_mprage_sag_p2",
        protocol: "T1 MPRAGE",
        image_type: "ORIGINAL\\PRIMARY\\M\\ND",
        acquisition: "3D",
        instances: 3,
        extra: &[],
    },
    // The same thing again in one session, which is what `run-` is for and
    // what a pick has to choose between.
    Series {
        key: "t1-mprage-repeat",
        study: "1",
        date: "20220115",
        description: "t1_mprage_sag_p2",
        protocol: "T1 MPRAGE",
        image_type: "ORIGINAL\\PRIMARY\\M\\ND",
        acquisition: "3D",
        instances: 3,
        extra: &[],
    },
    Series {
        key: "t2-flair",
        study: "1",
        date: "20220115",
        description: "t2_tirm_tra_dark-fluid",
        protocol: "T2 FLAIR",
        image_type: "ORIGINAL\\PRIMARY\\M\\ND",
        acquisition: "2D",
        instances: 3,
        extra: &[(0x0018, 0x0082, VR::DS, "2500")],
    },
    Series {
        key: "localizer",
        study: "1",
        date: "20220115",
        description: "localizer",
        protocol: "AAHead_Scout",
        image_type: "ORIGINAL\\PRIMARY\\M\\ND",
        acquisition: "2D",
        instances: 3,
        extra: &[],
    },
    Series {
        key: "screenshot",
        study: "1",
        date: "20220115",
        description: "Patient Protocol",
        protocol: "Report",
        image_type: "DERIVED\\SECONDARY\\SCREEN SAVE",
        acquisition: "2D",
        instances: 1,
        extra: &[],
    },
    Series {
        key: "dwi",
        study: "1",
        date: "20220115",
        description: "ep2d_diff_b1000_trace",
        protocol: "DWI",
        image_type: "ORIGINAL\\PRIMARY\\DIFFUSION\\NONE",
        acquisition: "2D",
        instances: 3,
        extra: &[],
    },
    Series {
        key: "adc",
        study: "1",
        date: "20220115",
        description: "ep2d_diff_b1000_ADC",
        protocol: "DWI",
        image_type: "DERIVED\\PRIMARY\\DIFFUSION\\ADC",
        acquisition: "2D",
        instances: 3,
        extra: &[],
    },
    // Susceptibility weighted, which BIDS has no word for.
    Series {
        key: "swi",
        study: "1",
        date: "20220115",
        description: "swi_tra_p2",
        protocol: "SWI",
        image_type: "ORIGINAL\\PRIMARY\\M\\ND",
        acquisition: "3D",
        instances: 3,
        extra: &[],
    },
    // Functional, which requires a task nobody has answered.
    Series {
        key: "bold",
        study: "1",
        date: "20220115",
        description: "ep2d_bold_moco",
        protocol: "BOLD",
        image_type: "ORIGINAL\\PRIMARY\\M\\ND\\MOSAIC",
        acquisition: "2D",
        instances: 3,
        extra: &[],
    },
    // The second occasion: a post-contrast repeat and two echoes.
    Series {
        key: "t1-post",
        study: "2",
        date: "20220715",
        description: "t1_mprage_sag_p2_KM",
        protocol: "T1 MPRAGE post",
        image_type: "ORIGINAL\\PRIMARY\\M\\ND",
        acquisition: "3D",
        instances: 3,
        extra: &[(0x0018, 0x0010, VR::LO, "GADOVIST")],
    },
    Series {
        key: "megre",
        study: "2",
        date: "20220715",
        description: "gre_me_tra",
        protocol: "ME GRE",
        image_type: "ORIGINAL\\PRIMARY\\M\\ND",
        acquisition: "2D",
        instances: 6,
        extra: &[],
    },
];

fn main() {
    let mut out = PathBuf::from("reference");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = PathBuf::from(args.next().unwrap_or_else(|| usage())),
            _ => usage(),
        }
    }
    fs::create_dir_all(&out).expect("create the output directory");

    let mut files = 0usize;
    for (n, s) in SERIES.iter().enumerate() {
        for i in 1..=s.instances {
            // The multi-echo series alternates its echo number, so it splits
            // into two stacks of three the way a real one does.
            let echo = match s.key {
                "megre" => match i % 2 {
                    1 => Some((1, 4.92)),
                    _ => Some((2, 9.84)),
                },
                _ => None,
            };
            let sop = format!("1.2.826.0.1.3680043.8.498.3.{}.{i}", n + 1);
            let path = out.join(s.key).join(format!("IM_{i:04}.dcm"));
            fs::create_dir_all(path.parent().expect("a parent")).expect("create");
            fs::write(&path, one(s, n, i, &sop, echo)).expect("write");
            files += 1;
        }
    }
    write_manifest(&out, files);
    println!(
        "reference: {} series, {files} files, under {}",
        SERIES.len(),
        out.display()
    );
}

fn one(s: &Series, n: usize, index: usize, sop: &str, echo: Option<(i64, f64)>) -> Vec<u8> {
    let study = format!("1.2.826.0.1.3680043.8.498.1.{}", s.study);
    // By position, because two keys of the same length are two series and a
    // UID built from a length is not a UID.
    let series = format!("1.2.826.0.1.3680043.8.498.2.{}", n + 1);
    let mut e = synth::minimal_mr(&study, &series, sop);
    e.extend([
        synth::text(tags::PATIENT_ID, VR::LO, "19800101-1234"),
        synth::text(tags::PATIENT_NAME, VR::PN, "REFERENCE^SUBJECT"),
        synth::text(tags::PATIENT_BIRTH_DATE, VR::DA, "19800101"),
        synth::text(tags::PATIENT_SEX, VR::CS, "F"),
        synth::text(tags::INSTITUTION_NAME, VR::LO, "A Synthetic Clinic"),
        synth::text(tags::STUDY_DATE, VR::DA, s.date),
        synth::text(tags::STUDY_TIME, VR::TM, "081500"),
        synth::text(tags::SERIES_DATE, VR::DA, s.date),
        synth::text(tags::SERIES_TIME, VR::TM, "082000"),
        synth::text(tags::SERIES_DESCRIPTION, VR::LO, s.description),
        synth::text(tags::PROTOCOL_NAME, VR::LO, s.protocol),
        synth::text(tags::IMAGE_TYPE, VR::CS, s.image_type),
        synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, s.acquisition),
        synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
        synth::text(tags::MANUFACTURER_MODEL_NAME, VR::LO, "Reference"),
        synth::text(tags::BODY_PART_EXAMINED, VR::CS, "BRAIN"),
        synth::text(tags::BURNED_IN_ANNOTATION, VR::CS, "NO"),
        synth::text(tags::MAGNETIC_FIELD_STRENGTH, VR::DS, "3.0"),
        synth::text(tags::SERIES_NUMBER, VR::IS, &format!("{}", n + 1)),
        synth::text(tags::INSTANCE_NUMBER, VR::IS, &index.to_string()),
        synth::text(tags::PATIENT_POSITION, VR::CS, "HFS"),
        // Geometry, so a converter can make a volume of it.
        synth::text(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, "1\\0\\0\\0\\1\\0"),
        synth::text(
            tags::IMAGE_POSITION_PATIENT,
            VR::DS,
            &format!("0\\0\\{}", index * 5),
        ),
        synth::text(tags::PIXEL_SPACING, VR::DS, "1.0\\1.0"),
        synth::text(tags::SLICE_THICKNESS, VR::DS, "5.0"),
        synth::us(tags::ROWS, 16),
        synth::us(tags::COLUMNS, 16),
        synth::us(tags::BITS_ALLOCATED, 16),
        synth::us(tags::BITS_STORED, 12),
        synth::us(tags::HIGH_BIT, 11),
        synth::us(tags::PIXEL_REPRESENTATION, 0),
        synth::us(tags::SAMPLES_PER_PIXEL, 1),
        synth::text(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
        synth::text(tags::REPETITION_TIME, VR::DS, "2000"),
        synth::text(tags::FLIP_ANGLE, VR::DS, "9"),
    ]);
    match echo {
        Some((number, time)) => e.extend([
            synth::text(tags::ECHO_NUMBERS, VR::IS, &number.to_string()),
            synth::text(tags::ECHO_TIME, VR::DS, &format!("{time}")),
        ]),
        None => e.push(synth::text(tags::ECHO_TIME, VR::DS, "3")),
    }
    for (group, element, vr, value) in s.extra {
        e.push(synth::text(dicom_core::Tag(*group, *element), *vr, value));
    }
    // The b value as the standard writes it, which is a double.
    if s.key == "dwi" {
        e.push(synth::num(dicom_core::Tag(0x0018, 0x9087), VR::FD, 1000.0));
    }
    e.push(synth::bytes(
        tags::PIXEL_DATA,
        VR::OW,
        vec![0x40u8; 16 * 16 * 2],
    ));
    synth::part10(&MetaFields::mr(sop), &e, true)
}

/// What the tree is, so the gate can say what it expected.
///
/// Deliberately not the answers themselves: those are checked into
/// `tools/release-check/reference.toml`, where a person can read them and a
/// change to the pack shows up as a difference rather than moving with the
/// code.
fn write_manifest(out: &Path, files: usize) {
    let mut text = String::from("{\n  \"series\": [\n");
    for (i, s) in SERIES.iter().enumerate() {
        text.push_str(&format!(
            "    {{\"key\": \"{}\", \"study\": \"{}\", \"date\": \"{}\", \"instances\": {}}}{}\n",
            s.key,
            s.study,
            s.date,
            s.instances,
            if i + 1 == SERIES.len() { "" } else { "," }
        ));
    }
    text.push_str(&format!(
        "  ],\n  \"files\": {files},\n  \"subjects\": 1\n}}\n"
    ));
    fs::write(out.join("reference.json"), text).expect("write the manifest");
}

fn usage() -> ! {
    eprintln!("usage: reference --out DIR");
    std::process::exit(2)
}
