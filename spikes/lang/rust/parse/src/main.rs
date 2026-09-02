// SPDX-License-Identifier: AGPL-3.0-only
//! The language spike, Rust side (docs/decisions/15, C1): walk a corpus, read every
//! file's DICOM header up to the pixel data, extract a fixed set of technical tags,
//! and write counts, rates and failure classes. Same semantics as `go/cmd/parse`.
//!
//! N worker threads parse; one writer thread owns the output files, which mimics v0's
//! single database writer. Output stays on the host: `index.tsv` holds one row per
//! parsed file keyed by a sequence number, `paths.tsv` maps sequence numbers to paths,
//! `failures.tsv` lists the files that failed with their class and the library's
//! message, and `summary.json` holds the numbers the report may quote.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::Parser;
use dicom_core::header::HasLength;
use dicom_core::Tag;
use dicom_dictionary_std::tags;
use dicom_object::collector::DicomCollectorOptions;
use dicom_object::file::ReadPreamble;
use dicom_object::{InMemDicomObject, OpenFileOptions};
use serde::Serialize;

const LIBRARY: &str = "dicom-object 0.10.0";

/// The technical tags the index keeps. Patient-level tags are never read.
const TAGS: &[(&str, Tag)] = &[
    ("SOPClassUID", tags::SOP_CLASS_UID),
    ("SOPInstanceUID", tags::SOP_INSTANCE_UID),
    ("StudyInstanceUID", tags::STUDY_INSTANCE_UID),
    ("SeriesInstanceUID", tags::SERIES_INSTANCE_UID),
    ("Modality", tags::MODALITY),
    ("Manufacturer", tags::MANUFACTURER),
    ("ManufacturerModelName", tags::MANUFACTURER_MODEL_NAME),
    ("SeriesDescription", tags::SERIES_DESCRIPTION),
    ("ProtocolName", tags::PROTOCOL_NAME),
    ("SeriesNumber", tags::SERIES_NUMBER),
    ("InstanceNumber", tags::INSTANCE_NUMBER),
    ("ImageType", tags::IMAGE_TYPE),
    ("EchoTime", tags::ECHO_TIME),
    ("RepetitionTime", tags::REPETITION_TIME),
    ("InversionTime", tags::INVERSION_TIME),
    ("FlipAngle", tags::FLIP_ANGLE),
    ("SliceThickness", tags::SLICE_THICKNESS),
    ("PixelSpacing", tags::PIXEL_SPACING),
    ("ImageOrientationPatient", tags::IMAGE_ORIENTATION_PATIENT),
    ("ImagePositionPatient", tags::IMAGE_POSITION_PATIENT),
    ("Rows", tags::ROWS),
    ("Columns", tags::COLUMNS),
];

#[derive(Parser, Debug)]
#[command(about = "NILS language spike: parse every file under a root and count")]
struct Args {
    /// Corpus root; every regular file below it is read, whatever its name
    #[arg(long)]
    root: PathBuf,
    /// Output directory (created); stays on the host
    #[arg(long)]
    out: PathBuf,
    /// Parser threads
    #[arg(long, default_value_t = 8)]
    workers: usize,
    /// Stop after this many files (0 = all)
    #[arg(long, default_value_t = 0)]
    limit: u64,
    /// Free-form label copied into summary.json
    #[arg(long, default_value = "")]
    label: String,
}

/// How a file ended up. The classes are the report's vocabulary, shared with Go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// Part 10 file, meta group present, header read to the pixel data
    Ok,
    /// No preamble and no meta group; read as a raw implicit VR little endian dataset
    OkRaw,
    /// Neither path produced a dataset with a SOP Instance UID: not a DICOM file
    NotDicom,
    /// The library gave up before the pixel data: a parse error
    ParseError,
    /// The file ended before the header did
    Truncated,
    /// Transfer syntax the library does not read
    UnsupportedTs,
    /// Parsed, but the dataset carries no SOP Instance UID
    MissingSop,
    /// The operating system refused the read
    IoError,
}

impl Class {
    fn name(self) -> &'static str {
        match self {
            Class::Ok => "ok",
            Class::OkRaw => "ok_raw",
            Class::NotDicom => "not_dicom",
            Class::ParseError => "parse_error",
            Class::Truncated => "truncated",
            Class::UnsupportedTs => "unsupported_ts",
            Class::MissingSop => "missing_sop",
            Class::IoError => "io_error",
        }
    }
}

struct Record {
    seq: u64,
    path: PathBuf,
    size: u64,
    class: Class,
    /// The library's message for failures, without the path
    message: String,
    /// Transfer syntax UID from the meta group, or "raw" for the fallback
    ts: String,
    values: Vec<String>,
}

#[derive(Serialize)]
struct Summary {
    implementation: &'static str,
    library: &'static str,
    label: String,
    workers: usize,
    host_cpus: usize,
    files: u64,
    bytes: u64,
    parsed: u64,
    failed: u64,
    classes: BTreeMap<String, u64>,
    transfer_syntaxes: BTreeMap<String, u64>,
    wall_seconds: f64,
    files_per_second: f64,
    megabytes_per_second: f64,
    user_cpu_seconds: f64,
    system_cpu_seconds: f64,
    peak_rss_megabytes: f64,
}

fn main() {
    let args = Args::parse();
    std::fs::create_dir_all(&args.out).expect("create out dir");
    let started = Instant::now();

    let (path_tx, path_rx) = crossbeam_channel::bounded::<(u64, PathBuf)>(4096);
    let (rec_tx, rec_rx) = crossbeam_channel::bounded::<Record>(4096);

    let workers: Vec<_> = (0..args.workers)
        .map(|_| {
            let rx = path_rx.clone();
            let tx = rec_tx.clone();
            std::thread::spawn(move || {
                for (seq, path) in rx.iter() {
                    if tx.send(parse_one(seq, path)).is_err() {
                        break;
                    }
                }
            })
        })
        .collect();
    drop(rec_tx);

    let out = args.out.clone();
    let writer = std::thread::spawn(move || write_all(&out, rec_rx));

    // The walker feeds the workers from this thread: no extension filter, no symlinks.
    let mut n: u64 = 0;
    for entry in walkdir::WalkDir::new(&args.root).follow_links(false) {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        n += 1;
        path_tx.send((n, entry.into_path())).expect("workers alive");
        if args.limit > 0 && n >= args.limit {
            break;
        }
    }
    drop(path_tx);
    for w in workers {
        w.join().expect("worker");
    }
    let (counts, ts_counts, bytes) = writer.join().expect("writer");

    let wall = started.elapsed().as_secs_f64();
    let (user, system, rss_kb) = rusage();
    let parsed =
        counts.get("ok").copied().unwrap_or(0) + counts.get("ok_raw").copied().unwrap_or(0);
    let summary = Summary {
        implementation: "rust",
        library: LIBRARY,
        label: args.label,
        workers: args.workers,
        host_cpus: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0),
        files: n,
        bytes,
        parsed,
        failed: n - parsed,
        classes: counts,
        transfer_syntaxes: ts_counts,
        wall_seconds: wall,
        files_per_second: n as f64 / wall,
        megabytes_per_second: bytes as f64 / 1e6 / wall,
        user_cpu_seconds: user,
        system_cpu_seconds: system,
        peak_rss_megabytes: rss_kb as f64 / 1024.0,
    };
    let json = serde_json::to_string_pretty(&summary).expect("json");
    std::fs::write(args.out.join("summary.json"), &json).expect("write summary");
    println!("{json}");
}

/// The writer thread: owns the three TSV files and the counters.
fn write_all(
    out: &Path,
    rx: crossbeam_channel::Receiver<Record>,
) -> (BTreeMap<String, u64>, BTreeMap<String, u64>, u64) {
    let mut index = BufWriter::new(File::create(out.join("index.tsv")).expect("index.tsv"));
    let mut paths = BufWriter::new(File::create(out.join("paths.tsv")).expect("paths.tsv"));
    let mut failures =
        BufWriter::new(File::create(out.join("failures.tsv")).expect("failures.tsv"));
    let mut header = vec!["seq", "size", "class", "ts"];
    header.extend(TAGS.iter().map(|(name, _)| *name));
    writeln!(index, "{}", header.join("\t")).unwrap();
    writeln!(paths, "seq\tpath").unwrap();
    writeln!(failures, "seq\tclass\tmessage\tpath").unwrap();

    let mut counts = BTreeMap::<String, u64>::new();
    let mut ts_counts = BTreeMap::<String, u64>::new();
    let mut bytes = 0u64;
    for rec in rx.iter() {
        bytes += rec.size;
        let class = rec.class.name();
        *counts.entry(class.to_string()).or_default() += 1;
        writeln!(paths, "{}\t{}", rec.seq, rec.path.display()).unwrap();
        match rec.class {
            Class::Ok | Class::OkRaw => {
                *ts_counts.entry(rec.ts.clone()).or_default() += 1;
                write!(index, "{}\t{}\t{}\t{}", rec.seq, rec.size, class, rec.ts).unwrap();
                for v in &rec.values {
                    write!(index, "\t{}", clean(v)).unwrap();
                }
                writeln!(index).unwrap();
            }
            _ => {
                writeln!(
                    failures,
                    "{}\t{}\t{}\t{}",
                    rec.seq,
                    class,
                    clean(&rec.message),
                    rec.path.display()
                )
                .unwrap();
            }
        }
    }
    index.flush().unwrap();
    paths.flush().unwrap();
    failures.flush().unwrap();
    (counts, ts_counts, bytes)
}

/// TSV cells carry no tabs, newlines or control characters.
fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

fn parse_one(seq: u64, path: PathBuf) -> Record {
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let mut rec = Record {
        seq,
        path,
        size,
        class: Class::Ok,
        message: String::new(),
        ts: String::new(),
        values: Vec::new(),
    };

    // A look at the first bytes decides which reader to use, the same look in both
    // languages: the DICM magic after the preamble or at the start means Part 10; a
    // group 0008 tag at the start means a raw dataset; anything else is not DICOM.
    match sniff(&rec.path) {
        Sniff::Part10 => {}
        Sniff::Raw => return parse_raw(rec),
        Sniff::Other => {
            rec.class = Class::NotDicom;
            return rec;
        }
        Sniff::Unreadable(e) => {
            rec.class = Class::IoError;
            rec.message = e.to_string();
            return rec;
        }
    }

    // The normal path: a Part 10 file with (or without) the 128-byte preamble, read
    // up to the pixel data.
    match OpenFileOptions::new()
        .read_preamble(ReadPreamble::Auto)
        .read_until(tags::PIXEL_DATA)
        .open_file(&rec.path)
    {
        Ok(obj) => {
            rec.ts = obj
                .meta()
                .transfer_syntax()
                .trim_end_matches('\0')
                .to_string();
            if obj.get(tags::SOP_INSTANCE_UID).is_none() {
                rec.class = Class::MissingSop;
            } else {
                rec.values = TAGS
                    .iter()
                    .map(|(_, tag)| value_str(obj.get(*tag)))
                    .collect();
            }
        }
        Err(err) => {
            let (class, message) = classify(&err);
            rec.class = class;
            rec.message = message.replace(&rec.path.display().to_string(), "<path>");
        }
    }
    rec
}

/// The fallback for a dataset without meta group: implicit VR little endian, the
/// default transfer syntax, read up to the pixel data.
fn parse_raw(mut rec: Record) -> Record {
    let raw = DicomCollectorOptions::new()
        .read_preamble(ReadPreamble::Never)
        .expected_ts("1.2.840.10008.1.2")
        .open_file(&rec.path)
        .and_then(|mut c| {
            let mut dset = InMemDicomObject::new_empty();
            c.read_dataset_up_to_pixeldata(&mut dset).map(|_| dset)
        });
    match raw {
        Ok(dset) if dset.get(tags::SOP_INSTANCE_UID).is_some() => {
            rec.class = Class::OkRaw;
            rec.ts = "raw".to_string();
            rec.values = TAGS
                .iter()
                .map(|(_, tag)| value_str(dset.get(*tag)))
                .collect();
        }
        Ok(_) => rec.class = Class::MissingSop,
        Err(err) => {
            let (class, message) = classify(&err);
            rec.class = class;
            rec.message = message.replace(&rec.path.display().to_string(), "<path>");
        }
    }
    rec
}

enum Sniff {
    Part10,
    Raw,
    Other,
    Unreadable(std::io::Error),
}

fn sniff(path: &Path) -> Sniff {
    let mut buf = [0u8; 132];
    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(e) => return Sniff::Unreadable(e),
    };
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) => return Sniff::Unreadable(e),
        }
    }
    if (n >= 132 && &buf[128..132] == b"DICM") || (n >= 4 && &buf[0..4] == b"DICM") {
        Sniff::Part10
    } else if n >= 8 && buf[0..2] == [0x08, 0x00] {
        Sniff::Raw
    } else {
        Sniff::Other
    }
}

fn value_str<D>(elem: Option<&dicom_core::DataElement<InMemDicomObject<D>>>) -> String
where
    D: dicom_core::dictionary::DataDictionary + Clone,
{
    match elem {
        Some(e) if !e.is_empty() => e.to_str().map(|s| s.into_owned()).unwrap_or_default(),
        _ => String::new(),
    }
}

/// Class from the error chain. Both readers (`OpenFileOptions`, `DicomCollector`)
/// have their own error enums with the same shapes, so the chain's messages and the
/// io error kind decide.
fn classify(err: &dyn std::error::Error) -> (Class, String) {
    let mut parts = vec![err.to_string()];
    let mut eof = false;
    let mut cur = err.source();
    while let Some(e) = cur {
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            eof |= io.kind() == std::io::ErrorKind::UnexpectedEof;
        }
        parts.push(e.to_string());
        cur = e.source();
    }
    let message = parts.join(" <- ");
    let top = &parts[0];
    let class = if top.starts_with("Could not open file") {
        Class::IoError
    } else if top.contains("transfer syntax") {
        Class::UnsupportedTs
    } else if eof || top.starts_with("Premature data set end") {
        Class::Truncated
    } else if top.starts_with("Could not read from file")
        || top.starts_with("Could not read preamble")
    {
        Class::IoError
    } else {
        Class::ParseError
    };
    (class, message)
}

fn rusage() -> (f64, f64, i64) {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    let tv = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    (tv(ru.ru_utime), tv(ru.ru_stime), ru.ru_maxrss as i64)
}
