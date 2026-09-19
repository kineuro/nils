// SPDX-License-Identifier: AGPL-3.0-only

//! What reading an enhanced multi-frame object whole costs (record 37, S8):
//! `cargo run --release -p nils-dicom --example frame-cost`, optionally with
//! frame counts as arguments. The archive holds 1.13 million frames over
//! 10,592 files, so the question is the cost per frame beyond the header the
//! reader already parses.
//!
//! Three times per shape: parsing the header alone, grouping its frames
//! alone, and the whole extraction. The file is synthetic and written to a
//! temporary directory; nothing reads a corpus.

use std::time::Instant;

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::frames::frame_groups;
use nils_dicom::synth::{self, Elem, TempDir};

/// How many times each measurement is repeated; the mean is printed.
const ROUNDS: u32 = 20;

fn main() {
    let counts: Vec<usize> = match std::env::args().skip(1).collect::<Vec<_>>() {
        args if args.is_empty() => vec![16, 120, 1_200],
        args => args.iter().filter_map(|a| a.parse().ok()).collect(),
    };
    let dir = TempDir::new("frame-cost");
    println!(
        "{:>7}  {:<12} {:>10} {:>10} {:>10} {:>9}",
        "frames", "shape", "read ms", "group ms", "extract ms", "us/frame"
    );
    for n in counts {
        for (shape, per_frame) in [("one stack", alike(n)), ("two stacks", split(n))] {
            let bytes = synth::part10(
                &synth::enhanced_meta("1.2.3.4"),
                &synth::enhanced_mr("1.2.3", "1.2.3.1", "1.2.3.4", Vec::new(), per_frame),
                true,
            );
            let path = dir.file("a.dcm", &bytes);
            let read = time(|| {
                nils_dicom::read(&path).expect("read");
            });
            let header = nils_dicom::read(&path).expect("read");
            let charset = nils_dicom::charset_of(&header.dataset);
            let values: Vec<Option<nils_dicom::Value>> = Vec::new();
            let group = time(|| {
                frame_groups(&header.dataset, &charset, &values);
            });
            let extract = time(|| {
                nils_dicom::extract(&path).expect("extract");
            });
            println!(
                "{n:>7}  {shape:<12} {read:>10.2} {group:>10.2} {extract:>10.2} {:>9.1}",
                group * 1_000.0 / n as f64
            );
        }
    }
}

/// Every frame the same: one group.
fn alike(n: usize) -> Vec<Vec<Elem>> {
    (0..n)
        .map(|_| vec![synth::fg_orientation("1\\0\\0\\0\\1\\0")])
        .collect()
}

/// Two orientations across the frames: two groups, as candidate Q's object.
fn split(n: usize) -> Vec<Vec<Elem>> {
    (0..n)
        .map(|i| {
            vec![
                synth::fg_orientation(match i < n / 2 {
                    true => "1\\0\\0\\0\\1\\0",
                    false => "0\\1\\0\\0\\0\\-1",
                }),
                synth::fg(
                    tags::MR_TIMING_AND_RELATED_PARAMETERS_SEQUENCE,
                    vec![synth::text(tags::REPETITION_TIME, VR::DS, "2500")],
                ),
            ]
        })
        .collect()
}

/// The mean of [`ROUNDS`] runs, in milliseconds.
fn time(mut f: impl FnMut()) -> f64 {
    f();
    let start = Instant::now();
    for _ in 0..ROUNDS {
        f();
    }
    start.elapsed().as_secs_f64() * 1_000.0 / f64::from(ROUNDS)
}
